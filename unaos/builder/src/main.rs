// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use std::process::{Command, Stdio};

mod vm_image;

fn main() {
    let workspace_dir = std::fs::canonicalize("..").unwrap();
    let target_dir = workspace_dir.join("target");
    let esp_dir = target_dir.join("x86_64_esp");

    println!("🔹 Building x86_64 Kernel...");
    let mut kernel_cmd = Command::new("cargo");
    kernel_cmd
        .current_dir(workspace_dir.join("crates/kernel"))
        .arg("+nightly")
        .arg("build")
        .arg("--release")
        .arg("--target").arg("../../x86_64-unaos.json")
        .arg("-Z").arg("build-std=core,compiler_builtins,alloc")
        .arg("-Z").arg("build-std-features=compiler-builtins-mem")
        .arg("-Z").arg("json-target-spec");
    // Optional kernel features from env knobs: UNAOS_SKIP_XHCI=1 (disable xHCI/USB bring-up),
    // UNAOS_BOOTLOG=1 (hold the boot log on screen instead of the GUI), UNAOS_USBDEBUG=1 (run the
    // USB main loop but keep the boot log on screen + print input events), UNAOS_PI=1 (Pi 4,
    // fbcon-only serial) and UNAOS_TEGRA=1 (Jetson Orin / Tegra234 UART). Composable. NOTE: keep
    // this list in sync with arroyo's feature mapping — the builder rebuilds the kernel, so a knob
    // missing here is silently dropped even if arroyo set it.
    let mut feats: Vec<&str> = Vec::new();
    // DEFAULT-QUIET: UNAOS_WITNESS=1 arms the `witness` fixture-battery feature so a headless `test`/`test-fat`
    // run re-proves the full x86 fixture set (nmi-self-fire/canonical-guard + U1a/U1b/U2-0a/U3/U3.5 + the
    // U2/U4x..U6bx storage chain that cascades U7x..U6gx). Default OFF => a default boot reaches the shell with
    // the boot-honesty lines only. arroyo auto-sets + EXPORTS it for the battery commands; kept in sync with arroyo.
    if std::env::var("UNAOS_WITNESS").is_ok() { feats.push("witness"); }
    // WXN-x86 M3b: UNAOS_WXNRO=1 arms `wxnro` — the W-clear on the kernel's executable pages. This
    // line is not optional bookkeeping: the ESP the x86 boot paths actually carry is THIS build, not
    // `arroyo`'s `build_kernel_x86_64` one, so a knob wired only in arroyo produces a media whose
    // `:: WXN-M3B: … ::` line honestly reads `armed=0` on a run the operator armed. (That is exactly
    // what the draft's first `UNAOS_WXNRO=1 ./arroyo test 60` printed, before this line existed — the
    // `armed=` field on the wire is what caught it, and it stays on the line for that reason.) Kept
    // in sync with arroyo.
    if std::env::var("UNAOS_WXNRO").is_ok() { feats.push("wxnro"); }
    // SELFHOST-2: UNAOS_SELFHOST=1 arms `selfhost` — the on-shard SRC.TGZ verify + tar walk. The
    // `test-selfhost` lane boots THIS build (the builder re-runs cargo itself), so a knob wired only
    // in arroyo would light the `⚡ kernel features:` banner while the kernel under test carried no
    // witness at all. Kept in sync with arroyo.
    if std::env::var("UNAOS_SELFHOST").is_ok() { feats.push("selfhost"); }
    if std::env::var("UNAOS_SKIP_XHCI").is_ok() { feats.push("skip_xhci"); }
    if std::env::var("UNAOS_BOOTLOG").is_ok() { feats.push("bootlog"); }
    // CLOCK-2: UNAOS_LOGTS=1 arms `logts` — a compact per-line timestamp prefix (monotonic ms → UTC
    // after a civil anchor) on the UART and both capture transports (FTDI capture ring, UNAOS.LOG).
    // Kept in sync with arroyo; missing here would be silently dropped.
    if std::env::var("UNAOS_LOGTS").is_ok() { feats.push("logts"); }
    if std::env::var("UNAOS_PI").is_ok() { feats.push("pi"); }
    if std::env::var("UNAOS_USBDEBUG").is_ok() { feats.push("usbdebug"); }
    // Wellspring raw-multitouch capture/decode (drivers/ehci §10g): the arroyo knob must survive
    // the builder's own feature derivation or the QEMU self-test never compiles in.
    // TPFRAME (B197): `UNAOS_MTRAW_INJECT` is retired — its injection is the default route now.
    if std::env::var("UNAOS_MTRAW").is_ok() { feats.push("mtraw"); }
    if std::env::var("UNAOS_SCHED_DEMO").is_ok() { feats.push("sched_demo"); }
    // UNAOS_IRQSTORAGE=1 routes x86 storage syscalls through the interrupt-driven storage service task
    // (STOR-1) instead of the staged-buffer path. x86_64 only; a no-op on the aarch64 media the arroyo
    // script builds. Metal-pending, so it stays opt-in.
    if std::env::var("UNAOS_IRQSTORAGE").is_ok() { feats.push("irqstorage"); }
    // UNAOS_BOTFAULT=1 injects ONE synthetic BOT failure (first WRITE(10), CSW stage) so the headless
    // suite exercises the xHCI BOT Reset Recovery path. Test-only; never on boot media.
    if std::env::var("UNAOS_BOTFAULT").is_ok() { feats.push("botfaultinject"); }
    // UNAOS_PFWIRE_SELFTEST=1 forces a fatal CPL-0 #PF from arch::init to prove the fault handlers put
    // their diagnostics on the wire (review §5/C2). BRICKS THE BOOT by design — test-only, never on
    // media. Mapped here as well as in `arroyo` so the QEMU `test` kernel (re-derived from env here)
    // actually compiles the witness in; a knob wired in arroyo alone would never reach it.
    if std::env::var("UNAOS_PFWIRE_SELFTEST").is_ok() { feats.push("pfwire_selftest"); }
    // ONSET-2 (M3): UNAOS_BOTRING64=1 grows the storage slot's two BULK transfer rings 16 -> 64 TRBs
    // (the one-variable wrap/Link discriminator). Default OFF => byte-identical media. It remains a
    // knob because it is a diagnostic, not a fix. MAPPED HERE AS WELL AS IN `arroyo` ON PURPOSE: a
    // knob wired into arroyo alone never reaches the ESP media the metal boot actually runs, which
    // has bitten this project twice — the boot log's `:: BOT: knobs … result=KNOBS ::` line reports
    // what really compiled in.
    // UNAOS_BOTCBWIOC is DELETED (2026-07-30): the CBW is awaited as its own stage in every build,
    // unconditionally, and no media can be produced with it off (usb_xhci.md §17).
    if std::env::var("UNAOS_BOTRING64").is_ok() { feats.push("botring64"); }
    // BOT-PARK: UNAOS_BOTWEDGE=1 injects a SYNTHETIC transport wedge on the storage slot once its
    // first 24 transactions have completed — every later BOT attempt fails `Timeout` with nothing
    // put on the wire. It exists because QEMU's usb-storage cannot wedge, so the retry ladder's
    // global floor (escalating back-off, the per-device retry budget, the park) is otherwise
    // walkable only on metal. Under it a boot reaches `:: BOT: PARKED … ::` and STOPS retrying,
    // which is the arc's whole claim. MAPPED HERE AS WELL AS IN `arroyo` for the reason BOTRING64
    // gives two knobs above, and it is not academic here: `arroyo test` compiles the booted x86
    // kernel THROUGH this builder, so while this line was missing `UNAOS_BOTWEDGE=1` armed nothing
    // and no run could reach the PARKED line the knob exists to produce.
    // TEST ONLY, never on media: it makes storage permanently unusable by design. Default OFF =>
    // fully cfg-compiled out and the artifact is byte-identical.
    if std::env::var("UNAOS_BOTWEDGE").is_ok() { feats.push("botwedge"); }
    // GR17 pay-as-you-go wc-g battery (video/wcg.rs): lattice-sampled first pass + deferred full
    // passes, x86-only paths, default OFF => byte-identical. Mapped here as well as in `arroyo`
    // for the same reason as BOTRING64 above: a knob arroyo alone sets never reaches boot media.
    if std::env::var("UNAOS_WCG_PAYGO").is_ok() { feats.push("wcg-paygo"); }
    // WCD-VALVE (boot-9 discriminator): suppress WC-D read-back admission under high composite
    // utilisation (video/wm.rs §WCD-VALVE). Mapped here as well as in `arroyo` for the reason the
    // knobs above state: a knob wired into arroyo alone never reaches the ESP media the metal boot
    // actually runs. Requires witness to reach anything; default OFF => byte-identical.
    if std::env::var("UNAOS_WCDVALVE").is_ok() { feats.push("wcdvalve"); }
    // LIVECON / QUARRY: the x86 desktop's live console window and file manager. Both were mapped
    // in `arroyo` alone — the two-place trap the KNOB→BUILDER check in arroyo now polices: a knob
    // this map does not read banners in the check while the boot media carries nothing. Default
    // OFF => byte-identical either way.
    if std::env::var("UNAOS_LIVECON").is_ok() { feats.push("livecon"); }
    if std::env::var("UNAOS_QUARRY").is_ok() { feats.push("quarry"); }
    // FACET (ORIN-FACET): the image viewer — `video/facet.rs`, opened by a double-click on a `.PNG`
    // row in the file manager. Mapped HERE as well as in `arroyo` because `facet` is named by the
    // literal `x86-all` type-check leg, which is exactly the condition arroyo's KNOB→BUILDER check
    // polices: a knob this map does not read arms the banner on x86 media carrying none of the code
    // (the `rastmc` failure that check was written for). Cargo's `facet = ["quarry"]` pulls the file
    // manager in behind it, so this one line is the whole wiring. Default OFF => byte-identical.
    if std::env::var("UNAOS_FACET").is_ok() { feats.push("facet"); }
    // VPERF: x86 video-path bench instrumentation (scroll/VRAM-read counters, fbmem readout,
    // display-BAR probe, scripted scroll scenario). x86_64-only module; default OFF.
    if std::env::var("UNAOS_VIDEOBENCH").is_ok() { feats.push("videobench"); }
    // RAST-1: software-rasterizer spinning-cube demo through the x86/virt panel path. x86_64-only
    // knob; default OFF => byte-identical media (the `rast` dep + demo module are unlinked).
    if std::env::var("UNAOS_RAST").is_ok() { feats.push("rast"); }
    // RASTPORT: the x86 MULTI-CORE rung (`rast_demo::run_mc`). Implies `rast` in Cargo, so this
    // alone arms both. MUST be listed here and not only in `arroyo`: this list is the one the x86
    // kernel that actually BOOTS is built from — `arroyo`'s `$KERNEL_FEATURES` does not reach it,
    // and a knob added there alone shows up in the banner while being absent from the image (which
    // is exactly how this was found: `rastmc` printed in the feature banner, `strings` on the ELF
    // had no `RAST-MC` in it, and the boot took the SCHED-X86 handoff that `rast` is supposed to
    // compile out). x86_64-only knob; default OFF => byte-identical media.
    if std::env::var("UNAOS_RASTMC").is_ok() { feats.push("rastmc"); }
    // PORTSW-1: the Panther Point EHCI->xHCI port switchover runs BY DEFAULT (metal-gated policy
    // 2026-07-16: the no-routing boot dropped ALL external USB on the 2012 rMBP). UNAOS_NOPORTSW=1
    // OPTS OUT (never-run no-routing experiment) => zero config-space writes, byte-identical no-routing
    // media; inert on QEMU (non-Intel xHCI). x86_64 only. Kept in sync with arroyo's mapping.
    if std::env::var("UNAOS_NOPORTSW").is_ok() { feats.push("noportsw"); }
    // EHCI-1 scout: UNAOS_EHCISCOUT=1 fires the STRICTLY READ-ONLY EHCI reconnaissance census probe
    // (dumps the EHCI companion controllers' cap/op/PORTSC state at boot; zero writes). `ehciscout_run`
    // gates only the pci.rs call site — the scout MODULE is compiled by default (the EHCI-3 driver
    // below is built from it). x86_64-only. Kept in sync with arroyo; also adds a QEMU `-device
    // usb-ehci` test target below.
    if std::env::var("UNAOS_EHCISCOUT").is_ok() { feats.push("ehciscout_run"); }
    // EHCI-2 configure-and-relook scout: UNAOS_EHCICONFIG=1 fires a knob-gated minimal EHCI wake
    // sequence + two PORTSC censuses (before/after CONFIGFLAG=1). `ehciconfig_run` gates only the call
    // site (implies ehciconfig for the wake it shares with the driver). Writes confined to the EHCI
    // functions' PMCSR/USBLEGSUP-OS-own/USBLEGCTLSTS/RS/CONFIGFLAG/PORTSC-port-power. Pair with
    // UNAOS_NOEHCIHID=1 for pure evidence (no driver). x86_64-only. Kept in sync with arroyo.
    if std::env::var("UNAOS_EHCICONFIG").is_ok() { feats.push("ehciconfig_run"); }
    // EHCI-4 M1: the EHCI-3 minimal HID driver (rMBP internal keyboard/trackpad) is now DEFAULT-ON on
    // x86 — metal-proven to type (usb_xhci.md §10). Push `ehcihid` (implies ehciconfig->ehciscout, and
    // the ACPI-root retention it uses) UNLESS opted out with UNAOS_NOEHCIHID=1, which unlinks the
    // module + every call site => byte-identical to the pre-fold no-EHCI media (PORTSW-1 policy).
    // Also moves the QEMU usb-kbd onto the harness ehci bus below by default so the driver has a
    // direct-path (Topology B) HID target. Kept in sync with arroyo's mapping.
    if std::env::var("UNAOS_NOEHCIHID").is_err() { feats.push("ehcihid"); }
    // KBDWIT: the one-shot per-endpoint EHCI interrupt-silence witness (drivers/ehci/mod.rs §KBDWIT),
    // for the s58 metal defect where the rMBP USB keyboard completed NOTHING all boot while the
    // trackpad on the same TT streamed. DEFAULT-ON for this round — a new witness family rides the
    // default boot only while it is earning its verdict — and suppressed by UNAOS_NOKBDWIT=1, which
    // unlinks the probe, its `IntEp` fields and its call site => the EHCI service path is
    // byte-identical to the pre-arc default. Gated on the SAME condition as `ehcihid`: `kbdwit`
    // deliberately does not IMPLY `ehcihid` (that would resurrect the driver for an operator who
    // opted out), so pushing it without the driver would be a feature with no module to compile
    // into. THIS list is what reaches the kernel binary for MEDIA builds — a knob mapped in arroyo
    // but missing here ships the feature DISABLED while the banner claims it is on (the s42/INSTGUI
    // and GMUX-IGD lesson, and the reason this line is not optional). Kept in sync with arroyo.
    if std::env::var("UNAOS_NOEHCIHID").is_err() && std::env::var("UNAOS_NOKBDWIT").is_err() {
        feats.push("kbdwit");
    }
    // BATMON-1: the Apple SMC battery monitor (x86_64). UNAOS_SMC=1 arms the polled SMC key/value
    // driver; the QEMU isa-applesmc device is attached below under the same knob so the protocol
    // machinery is gated by a known-key read. Kept in sync with arroyo's mapping.
    if std::env::var("UNAOS_SMC").is_ok() { feats.push("smc"); }
    // USBLUN (orin 27, M3/F2): the way back from the USB multi-LUN census in `drivers/xhci/mod.rs`.
    // The census is DEFAULT-ON (it is the driver doing its job — a multi-slot card reader is one BOT
    // device whose card slots are logical units), so this knob only ever turns it OFF:
    // UNAOS_NOUSBLUN=1 => no census compiled, Get Max LUN never asked, `bCBWLUN = 0` and LUN 0
    // published exactly as before. MUST be listed here and not only in `arroyo`: THIS list is the one
    // the x86 kernel that actually boots is built from, and a knob mapped there alone is the rastmc
    // failure — the banner says the feature is on and the image carries none of it. Arch-neutral
    // feature; the x86 image is the one `./arroyo test` boots and the one the rMBP bench boots.
    // Kept in sync with arroyo's mapping.
    if std::env::var("UNAOS_NOUSBLUN").is_ok() { feats.push("nousblun"); }
    // SDWRITE (A60): the native root's write POSTURE — `fs/vfs.rs`'s `NativeBackend` write-veto
    // forward and, under `witness`, leg 8 of the boot-path battery
    // (`fs::bootdisk::sdwrite_posture_selftest`, which prints `:: SDWRITE-POSTURE: posture=…`).
    // DEFAULT-ON, opted out with UNAOS_NOSDWRITE=1 — the same polarity `arroyo:1992` has carried
    // unconditionally since the orin session, because `sdwrite` is meant to ride EVERY image.
    //
    // CAUGHT BY BANNERCERT (`scripts/banner-cert.sh`) ON ITS FIRST ARMED RUN, 2026-09-15: this is
    // the rastmc failure named just above, and the s42/INSTGUI and GMUX-IGD lesson, a THIRD time —
    // and the first one a gate found instead of a person. arroyo put `sdwrite` on the
    // `⚡ kernel features:` banner of every verb; THIS list, the one the x86 kernel that actually
    // boots is built from, had no entry for it. A single `esp-x86` run printed both lists and they
    // differed by exactly this name (arroyo's ended `…,gmux_igd,sdwrite`, 28 names; this builder's
    // own `   kernel features:` ended `…,gmux_igd,smolnet`, 27), and the staged ELF carried
    // `SDWRITE-POSTURE` 0 times while `:: USBREG` — printed from the SAME `witness`-gated
    // `unafsroot_selftest` — appeared 3 times, so the enclosing code was live and the feature simply
    // was not compiled in. Every x86 media image cut since A60 landed shipped without it while the
    // build log said otherwise. Kept in sync with arroyo's mapping; `esp-x86` now reds on exactly
    // this class instead of announcing the media.
    if std::env::var("UNAOS_NOSDWRITE").is_err() { feats.push("sdwrite"); }
    // WALK-QUIET (GR18): UNAOS_SMCWALK=1 restores the #KEY index walk's PER-NAME output. The walk and
    // its one-line summary are always-on under `smc`; this buys back the 493-line inventory dump that
    // Boot V measured at ~3.5 s of displaced storage bring-up. Does NOT imply `smc` — inert without
    // it. Kept in sync with arroyo's mapping; a knob mapped there and missing HERE ships the feature
    // disabled while the banner claims it is on.
    if std::env::var("UNAOS_SMCWALK").is_ok() { feats.push("smcwalk"); }
    // SDHC-4a: UNAOS_SDW=1 arms the CMD24 single-block WRITE path on the built-in PCIe SD reader
    // (drivers/sdhc.rs). THIS list is what reaches the kernel binary for MEDIA builds — a knob mapped
    // in arroyo but missing here ships the feature DISABLED while the operator believes it is armed,
    // which for a WRITE arm is the most consequential version of that bug in the tree: the boot would
    // print `armed=0 ... -> DRYRUN` on a run the operator armed, and (had the field not been on the
    // wire) would have looked like a card that refused. The `armed=` field exists for exactly this,
    // and it is what caught the same omission in WXN-M3b. Kept in sync with arroyo's mapping.
    // SDHCRW (rmbp-ledger B166, R59, 2026-09-22): DEFAULT-ON now — R59 is "read write" and LAWS §3 is
    // default-on with a named opt-out, so the ladder rides every x86 MEDIA image, not only a knobbed
    // one. Unconditional rather than `UNAOS_NOSDW`-guarded on purpose: the opt-out is a POSTURE knob
    // (`sdw-ro` below), and an image that lost the ladder as well would refuse with
    // `reason=no-write-path` — "this build cannot write" — when the truth is "this build was told
    // not to". A default-on name missing HERE is the `sdwrite` class, which `scripts/knob-parity.sh`
    // reds on; UNAOS_SDW=1 remains accepted upstream in arroyo and simply decides nothing now.
    feats.push("sdw");
    // SDHCRW (rmbp-ledger B166, R59): UNAOS_SDW_RO=1 — the NAMED OPT-OUT, the inversion of B155's
    // UNAOS_SDW_RW. Mapped HERE as well as in arroyo for the reason the `sdw` line above spells out,
    // and this knob is the most consequential case of it in the tree: mapped in arroyo alone, an
    // operator arming a COLD-WITNESS boot on metal would get a banner saying `sdw-ro` and a card
    // that writes — the exact inverse of B155's worry and a worse one, because the failure is a
    // mutation that happened rather than a capture that did not. The truth-table line
    // (`:: SDHCPOST: posture sdw-ro=… ::`) prints in BOTH polarities so this omission is caught on
    // the wire instead of on the card.
    if std::env::var("UNAOS_SDW_RO").is_ok() { feats.push("sdw-ro"); }
    // SDHC-4b (GR20): UNAOS_SDHCBLK=1 makes the INTERNAL SD card a real x86 block backend, published
    // under its OWN registry handle (`BlockHandle::Sdhc`) so `fs::fat` can mount it READ-ONLY without
    // the boot volume — the USB stick this machine boots from — moving at all. THIS list is what
    // reaches the kernel binary for MEDIA builds, so a knob wired into arroyo alone would ship the
    // backend disabled while the `⚡ kernel features:` banner claimed it was on (s42/INSTGUI, WXN-M3b).
    // The failure would be quiet in the worst way here: no `:: SDHCBLK: registered … ::` line and no
    // mount witness reads exactly like "no card was identified", which is a different finding.
    //
    // BOOT-STORAGE (GR26): DEFAULT-ON, opted out with UNAOS_NOSDHCBLK=1. The opt-in default was a
    // GR20 decision taken when the rMBP booted from a USB card reader and the internal slot was a
    // second, optional source. That premise is gone: the bench machine now boots from a SINGLE SD
    // card in its INTERNAL slot, so the internal reader is the ONLY program source there is, and an
    // opt-in knob makes the default x86 image one that cannot reach its own boot volume. GR26 Boot D
    // is the conviction — the card was identified, read-verified 3/3 windows and MBR-checked, and
    // then every consumer printed `handles=global=absent sdhc=unbuilt` and declined, because THIS
    // line had not fired. Turning it on costs a READ-ONLY third handle: `register_sdhc` never touches
    // the global slot, and `default_writable`'s substitution guard fails OPEN in all but the
    // positively-proven case.
    //
    // REVIEW CORRECTION (GR26): an earlier wording of this comment said "`fs::fat` refuses every
    // write to a `Sdhc` source". That was GR20's property and SDHC-4c REPLACED it — `fs/fat.rs`'s
    // write path now admits a span that lies inside the reserved extent (`fs::sdhc4c::permit_write`,
    // fat.rs:705-709/836-853), and `drivers/block.rs` §SDHC-4c says so at the seam. What actually
    // makes a DEFAULT image read-only on the card is the absence of `sdw`, which is unchanged by
    // this flip and still opt-in: without it the image carries no CMD24 ladder at all, so
    // `block::write_block_sdhc` is the refusing stub (`no `sdw` feature … no CMD24 ladder`,
    // block.rs:1276-1285) and `FatFs::sdhc4c_write_verify` is the SKIP stub (fat.rs:4155-4163). Say
    // it that way round, because the two statements fail differently: `UNAOS_SDW=1` alone now also
    // arms `sdhcblk`, which it did not before this flip, so an `sdw` build reaches the SDHC-4c
    // reserve pass on the internal card without a second knob.
    //
    // Kept in sync with arroyo's mapping.
    if std::env::var("UNAOS_NOSDHCBLK").is_err() { feats.push("sdhcblk"); }
    // PCI-CENSUS (GR20): UNAOS_PCICENSUS=1 arms the complete READ-ONLY PCI enumeration witness
    // (arch/x86_64/pci.rs::full_census) — one `[PCI-CENSUS]` line per function present, plus a
    // capability dump per network-class function. THIS list is what reaches the kernel binary for
    // MEDIA builds: the builder re-derives the x86 feature set from env, so a knob wired into
    // arroyo alone ships the census DISABLED while the `⚡ kernel features:` banner claims it is on
    // — the s42/INSTGUI and WXN-M3b failure, and the one this arc is most exposed to, because a
    // census that silently did not run is indistinguishable on the wire from a machine with
    // nothing on its buses. Config reads only, no BAR sizing, bounded sweep. Default OFF =>
    // function + call site unlinked, media byte-identical. Kept in sync with arroyo's mapping.
    if std::env::var("UNAOS_PCICENSUS").is_ok() { feats.push("pcicensus"); }
    // AHCI (rmbp-ledger B89, first rung): UNAOS_AHCI=1 arms drivers/ahci.rs — the READ-ONLY SATA host
    // controller driver. THIS list is what reaches the kernel binary for MEDIA builds: the builder
    // re-derives the x86 feature set from env, so a knob wired into arroyo alone ships the driver
    // DISABLED while the `⚡ kernel features:` banner claims it is on (the s42/INSTGUI and WXN-M3b
    // failure). This arc is as exposed to that bug as the PCI census was and in the same direction:
    // a driver that silently did not run prints no `:: AHCI: ... ::` witness, which is byte-for-byte
    // what a machine with no SATA controller in it looks like on the wire — a different finding, and
    // on the rMBP the WRONG one, since the census has named the controller on every capture.
    // READ-ONLY: the driver compiles exactly IDENTIFY DEVICE (0xEC) and READ DMA EXT (0x25), no ATA
    // write opcode, and `install/` is never told the handle exists (B91 — Catalina lives on that SSD).
    // Default OFF => module unlinked, media byte-identical. Kept in sync with arroyo's mapping.
    if std::env::var("UNAOS_AHCI").is_ok() { feats.push("ahci"); }
    // AHCIWRITE (rmbp-ledger B89, SATA write half): UNAOS_AHCI_WRITE=1 arms the ATA `WRITE DMA EXT`
    // (0x35) path. THIS list is what reaches the kernel binary for MEDIA builds, and this is the one
    // knob in it that can destroy data on Peter's internal SSD, so the s42/INSTGUI failure mode is
    // the worst it could be here: wired in arroyo alone the banner would claim the write path is
    // armed while the image refused every write, and the operator would read a working install as a
    // broken driver — or, the other way round on a later run, trust a banner that was never true.
    // `ahci-write` implies `ahci` in Cargo.toml, so this push alone is sufficient; the flight lines
    // still name both env knobs because a reader should see both on a line that can write a disk.
    // Default OFF => no ATA write opcode is linked (`WRITE-DMA-EXT-0x35` is 0 hits on the ELF) and
    // media are byte-identical. Kept in sync with arroyo's mapping and crates/kernel/Cargo.toml.
    if std::env::var("UNAOS_AHCI_WRITE").is_ok() { feats.push("ahci-write"); }
    // HDA (rmbp-ledger B127, arc 1): UNAOS_HDA=1 arms drivers/hda.rs — the High Definition Audio
    // controller, the kernel's first audio line. THIS list is what reaches the kernel binary for
    // MEDIA builds and for every QEMU run that goes through this builder: the builder re-derives
    // the x86 feature set from env, so a knob wired into arroyo alone ships the driver DISABLED
    // while the `⚡ kernel features:` banner claims it is on (the s42/INSTGUI and WXN-M3b failure).
    // This arc is as exposed to that bug as the PCI census and AHCI were, and in the same
    // direction: a driver that silently did not run prints no `[hda]` line at all, which is
    // byte-for-byte what a machine with no audio controller in it looks like on the wire — and on
    // BOTH targets that reading would be wrong, since the rMBP's PCH carries one and the QEMU
    // fixture attached below IS one. Polled end to end; INTCTL is never written.
    // Default OFF => module unlinked, census and hook cfg-erased, media byte-identical.
    // Kept in sync with arroyo's mapping and crates/kernel/Cargo.toml.
    if std::env::var("UNAOS_HDA").is_ok() { feats.push("hda"); }
    // HDATONE (rmbp-ledger B127, arc 2): UNAOS_HDATONE=1 arms the output stream — a 440 Hz sine on
    // output stream 0. `hda-tone` implies `hda` in Cargo.toml, so this push alone is sufficient;
    // the flight lines still name both env knobs because a reader should see both on a line that
    // makes an audible noise. THIS list is what reaches the kernel binary for MEDIA builds, so a
    // knob wired into arroyo alone would put `hda-tone` in the banner over a kernel with the whole
    // tone block compiled OUT — and for this arc that failure reads on the wire as a codec that
    // would not start, which is the opposite conclusion from the true one.
    // Default OFF => the tone block, the sine generator and every stream write unlinked, media
    // byte-identical. Kept in sync with arroyo's mapping and crates/kernel/Cargo.toml.
    if std::env::var("UNAOS_HDATONE").is_ok() { feats.push("hda-tone"); }
    // HDASIE (rmbp-ledger B207): UNAOS_HDASIE=1 sets INTCTL.SIE for the tone's one descriptor before RUN
    // and restores it after STOP — the BCIS-latch experiment B130 left open. Implies `hda-tone` in
    // Cargo.toml. Same reason as HDATONE for living in THIS list: a media build must carry the bit or
    // the wire reads `wrote-intctl=0(audited)` and the flight answers nothing. Default OFF, byte-identical.
    if std::env::var("UNAOS_HDASIE").is_ok() { feats.push("hda-sie"); }
    // BCMA-RECON (GR20): UNAOS_BCMARECON=1 arms drivers/bcma.rs — STRICTLY READ-ONLY recon of the
    // Broadcom WiFi radio (class 0x02 / subclass 0x80), the first arc of the native-BCM4331 path.
    // THIS list is what reaches the kernel binary for MEDIA builds: the builder re-derives the x86
    // feature set from env, so a knob wired into arroyo alone ships the probe DISABLED while the
    // `⚡ kernel features:` banner claims it is on (the s42/INSTGUI and WXN-M3b failure). This arc is
    // as exposed to that bug as the census was, and in the same direction: a recon that silently did
    // not run is indistinguishable on the wire from a machine with no radio in it — which is exactly
    // the conclusion the whole path-A decision would then be built on. Config reads + BAR0 reads
    // only; no config write, no register write, no BAR sizing. Default OFF => module + call site
    // unlinked, media byte-identical. Kept in sync with arroyo's mapping.
    if std::env::var("UNAOS_BCMARECON").is_ok() { feats.push("bcmarecon"); }
    // BCMA-S1 (GR20): UNAOS_BCMAS1=1 arms the WiFi path's FIRST WRITE — one PCI config write to
    // cfg:0x80 (BCMA_PCI_BAR0_WIN) pointing the BAR0 window at ChipCommon, reading chip id + EROM,
    // then RESTORING the recorded pre-image (never the assumed enumeration base). It rides on top of
    // the recon: the `bcmaS1` cargo feature implies `bcmarecon`, so pushing `bcmaS1` alone here pulls
    // in S0 and the x86-only module gate + call site. THIS list is what reaches the kernel binary for
    // MEDIA builds, so a knob wired into arroyo alone would ship the write DISABLED while the banner
    // claims it is on (the s42/INSTGUI and WXN-M3b failure) — as exposed as the census was and in the
    // same direction. Default OFF => module + call site unlinked, media byte-identical. Kept in sync
    // with arroyo's mapping.
    if std::env::var("UNAOS_BCMAS1").is_ok() { feats.push("bcmaS1"); }
    // WIFI-1 (GR25): UNAOS_WIFI=1 arms src/wifi/ — the BCM4331 FIRMWARE-LOAD path. Config-space
    // identification of the AirPort radio (class 0x02 / subclass 0x80), cross-checked against the
    // metal facts bcm4331.md §0 pinned, then the user-supplied firmware SET located, validated and
    // staged off the program-source FAT volume. THIS list is what reaches the kernel binary for
    // MEDIA builds: the builder re-derives the x86 feature set from env, so a knob wired into arroyo
    // alone ships the loader DISABLED while the `⚡ kernel features:` banner claims it is on (the
    // s42/INSTGUI and WXN-M3b failure, and the one bcm4331.md §4 calls "not optional"). This arc is
    // exposed in the same direction as the recon: a loader that silently did not run is
    // indistinguishable on the wire from media with no firmware on it. Config reads + FAT reads
    // only; no config write, no register write, no MMIO. Default OFF => module + call sites
    // unlinked, media byte-identical. Kept in sync with arroyo's mapping.
    if std::env::var("UNAOS_WIFI").is_ok() { feats.push("wifi"); }
    // WIFI-2 (GR25): UNAOS_WIFI2=1 arms arc 2 — the WRITE rungs (src/wifi/bringup.rs). Maps BAR0,
    // moves the backplane window selector cfg:0x80 onto ChipCommon and then onto the enumeration ROM,
    // walks the core table from our own reads, cross-checks the d11 core against four metal boots and
    // against the two config registers firmware left behind, reads the core + wrapper state and
    // re-measures bcm4331.md §S3's enable rule (a no-op on this machine, and the branch that is not
    // makes only the REVERSIBLE half — reset is never asserted). The microcode upload is refused at a
    // named UNKNOWN (§S4 gives no value for the B43_SHM_UCODE routing selector, and the source that
    // does is off-limits for src/wifi/). Implies `wifi`. The builder wiring is not optional and is
    // exposed in exactly the direction the census was: media built here re-derives the x86 feature set
    // from ITS OWN env, so a knob wired only in arroyo ships arc 2 disabled while the banner claims it
    // is on — and a bring-up that silently did not run is indistinguishable on the wire from a radio
    // that would not answer. Default OFF => module unlinked, media byte-identical to the arc-1 build.
    // Kept in sync with arroyo's mapping.
    if std::env::var("UNAOS_WIFI2").is_ok() { feats.push("wifi2"); }
    // WVAL-REPLAY: UNAOS_WIFIVAL=1 arms the census-ABSENT REPLAY leg — the QEMU-reachable half of
    // arc 2. QEMU models no BCM4331, so the census refuses at S_START and the module parks, which
    // left the FAT search, the bounds checks, `classify_header`'s container verdict and arc 2's
    // set-validation dry-run with exactly one gate: a bench round. Under this knob the ABSENT branch
    // prints a REPLAY-armed witness, proceeds to the storage wait, stages the set off the media, and
    // runs `bringup::validate_replay()` — the completeness gate, `validate_set()`, one park line, and
    // no PCI access, no BAR map, no window-selector move and no core walk anywhere in its call graph.
    // Implies `wifi2`. THIS list is what reaches the kernel binary for MEDIA builds — the builder
    // re-derives the x86 feature set from ITS OWN env — so a knob wired into arroyo alone would ship
    // the replay leg DISABLED while the `⚡ kernel features:` banner claims it is on (the s42/INSTGUI
    // and WXN-M3b failure), and the failure mode here is the nastiest shape of it: the spec gate
    // would go red on a kernel that never contained the code, and read as a classifier regression.
    // Default OFF => the ABSENT branch parks exactly as before, media byte-identical. Kept in sync
    // with arroyo's mapping.
    if std::env::var("UNAOS_WIFIVAL").is_ok() { feats.push("wifival"); }
    // WIFI-3: UNAOS_WIFI3=1 arms arc 3's UPLOAD rung — the bcm4331 microcode upload
    // (`upload_ucode` in src/wifi/bringup.rs). W5 pinned the SHM routing gate 3 refused on
    // (0x0300 = microcode memory, control word 0x03000000; the b43 open specification,
    // bcm-specs.sipsolutions.net — see bcm4331.md §S4-W5), so the default refusal now says
    // reason=wifi3-not-armed. DESTRUCTIVE on metal: the prologue's core reset destroys the
    // resident microcode (bcm4331.md §5 risk 4); only a successful upload + handshake
    // restores a working state. Implies `wifi2`. THIS list is what reaches the kernel binary
    // for MEDIA builds — the builder re-derives the x86 feature set from ITS OWN env — so a
    // knob wired into arroyo alone would ship the upload DISABLED while the banner claims it
    // is on (the s42/INSTGUI and WXN-M3b failure), and on THIS feature that shape is the
    // worst one available: a boot that made the destructive prologue impossible while the
    // operator believed the upload was armed. Default OFF => module unlinked, media
    // byte-identical. Kept in sync with arroyo's mapping.
    if std::env::var("UNAOS_WIFI3").is_ok() { feats.push("wifi3"); }
    // WIFI-4: UNAOS_WIFI4=1 arms arc 4's PHY/RADIO rung — bcm4331.md §S5(a) the radio identity
    // register and §S5(b) the PHY's post-upload liveness (`phy_once` in src/wifi/bringup.rs). ONE
    // device write, an indirect-window SELECTOR on the read path (d11+0x3F6, the radio-register
    // address port both spec generations pin), pre-image restored; the radio DATA ports are never
    // written. Not destructive, and GATED on the same boot's wifi3 `-> UPLOADED` verdict — without
    // it the rung refuses on the wire and touches nothing. Implies `wifi3`. THIS list is what
    // reaches the kernel binary for MEDIA builds — the builder re-derives the x86 feature set from
    // ITS OWN env — so a knob wired into arroyo alone would ship the rung DISABLED while the banner
    // claims it is on (the s42/INSTGUI and WXN-M3b failure), and here that shape has its own sting:
    // the operator would fly the DESTRUCTIVE wifi3 boot believing it also bought the §S5 rung, and
    // the absence of `:: wifi4:` lines is indistinguishable from a radio that never answered.
    // Default OFF => rung unlinked, media byte-identical. Kept in sync with arroyo's mapping.
    if std::env::var("UNAOS_WIFI4").is_ok() { feats.push("wifi4"); }
    // BT-L0 (GR21): UNAOS_BT=1 arms the first Bluetooth arc — "does the radio answer?". Lifts the
    // EHCI hub-walk depth cap 2 -> 3 to reach the HCI controller behind the FULL-SPEED Broadcom hub
    // `0a5c:4500`, and — in the SAME change, because either alone is wrong — fixes the
    // split-transaction TT computation so a device below a non-high-speed hub inherits the nearest
    // HIGH-SPEED ancestor's TT (USB 2.0 §11.14) instead of its immediate parent's. Then recognizes
    // interface class 0xE0/0x01/0x01 and issues HCI_Reset (0x0C03) + HCI_Read_Local_Version (0x1001)
    // over the CONTROL endpoint, reading the replies off the INTERRUPT-IN event endpoint. No bulk,
    // no async schedule (PROBE-14: this Panther Point's async engine master-aborts); every wait
    // bounded. `bt` implies `ehcihid`, which this list already pushes by default, so pushing `bt`
    // alone is the whole delta. THIS list is what reaches the kernel binary for MEDIA builds, so a
    // knob wired into arroyo alone would ship Bluetooth DISABLED while the banner claims it is on
    // (the s42/INSTGUI and WXN-M3b failure). Default OFF => cap, TT fix and the whole L0 sequence
    // unlinked, media byte-identical. Kept in sync with arroyo's mapping.
    if std::env::var("UNAOS_BT").is_ok() { feats.push("bt"); }
    // BT-C1 (GR24): UNAOS_BTC=1 arms the first BR/EDR step toward A2DP audio — HCI_Create_Connection
    // (0x0405) pages the speaker at the BD_ADDR in bt_name.rs, witnesses Connection Complete (event
    // 0x03) or the failure status, and releases the link (or cancels an unresolved page). Its own
    // knob, and NOT part of UNAOS_BT, because a page is a directed transmission that makes an
    // audible noise on the speaker: a boot that did not ask for one must be structurally incapable
    // of issuing one. `btc` implies `bt` in Cargo.toml, so pushing `btc` alone arms the whole BT
    // stack. THIS list is what reaches the kernel binary for MEDIA builds, so a knob wired into
    // arroyo alone would ship the page DISABLED while the banner claims it is on (the s42/INSTGUI
    // and WXN-M3b failure). Default OFF => the page code and its constants unlinked, media
    // byte-identical. Kept in sync with arroyo's mapping.
    if std::env::var("UNAOS_BTC").is_ok() { feats.push("btc"); }
    // BT-DIR: UNAOS_BTDIR=1 arms THE DIRECTION TEST — after the outbound page stage has printed its
    // tally, write HCI_Write_Scan_Enable (0x0C1A) = 0x03 so the peer can page THIS host, hold one
    // 6400 ms page window, report whether a Connection Request (event 0x04) arrived, then write
    // 0x00 back and read it back. Its own knob, and NOT part of UNAOS_BTC, because the outbound
    // train is this arc's CONTROL: a controller with page scan enabled time-slices between inbound
    // scan windows and any outbound train, so a `btc` build must stay byte-identical to the builds
    // the control was measured on. It is also a distinct air-side posture — the machine is
    // DISCOVERABLE AND CONNECTABLE for the length of that window. `btdir` implies `btc` in
    // Cargo.toml, so pushing `btdir` alone arms the page and the whole BT stack. THIS list is what
    // reaches the kernel binary for MEDIA builds, so a knob wired into arroyo alone would ship the
    // direction test DISABLED while the banner claims it is on (the s42/INSTGUI and WXN-M3b
    // failure), which for this arc would mean recording a silence produced by absent code as a
    // silence produced by the radio. Default OFF => bt_dir_probe, its constants and its call site
    // unlinked, media byte-identical. Kept in sync with arroyo's mapping.
    if std::env::var("UNAOS_BTDIR").is_ok() { feats.push("btdir"); }
    // BT-BOND M1 / HOLOCRON: UNAOS_HOLOCRON=1 arms the kernel-side classed-record store
    // (`src/fs/holocron.rs`) and its first client, the bond record codec + table
    // (`src/drivers/ehci/btbond.rs`). THIS list is what reaches the kernel binary for MEDIA builds
    // and for every QEMU run that goes through the builder, so a knob wired into arroyo alone would
    // put `holocron` in the `⚡ kernel features:` banner over a kernel with both modules compiled
    // OUT — the s42/INSTGUI and WXN-M3b failure, and the exact reason this arc's gate proves the
    // witness family with `strings` against the builder-path artifact rather than trusting the
    // banner. M1 issues no HCI command and touches no radio. Default OFF => modules and call sites
    // unlinked, media byte-identical. Kept in sync with arroyo's mapping.
    if std::env::var("UNAOS_HOLOCRON").is_ok() { feats.push("holocron"); }
    // LOGIN (RULINGS R51): UNAOS_LOGIN=1 arms the human-user line — fs/users.rs (the USERS.DAT record
    // store), the session principal in both syscall.rs, the login/logout shell arms and the M1 fixture.
    // Kept in sync with arroyo's map (the knob→builder wiring gate holds this line to it).
    if std::env::var("UNAOS_LOGIN").is_ok() { feats.push("login"); }
    // LOGIN SELFTESTS (its own knob, the UNAOS_HCRONST rule): the fixtures, never on a shipping login boot.
    if std::env::var("UNAOS_LOGINST").is_ok() { feats.push("loginst"); }
    // BT-BOND M1 / HOLOCRON SELFTESTS: UNAOS_HCRONST=1 arms the store's two BOOT-TIME-WRITE selftests
    // (`holocron::selftest_once`, `btbond::selftest_once`). Its own knob and NOT part of
    // UNAOS_HOLOCRON, by the same rule that gives `sdw` a knob apart from `sdhcblk`: a boot that did
    // not ask to WRITE the boot medium must be incapable of doing so. Implies `holocron` in
    // Cargo.toml, so pushing this alone arms both. THIS list is what reaches the kernel binary for
    // MEDIA builds and for every QEMU run that goes through the builder, so a knob wired into arroyo
    // alone would put `hcronst` in the `⚡ kernel features:` banner over a kernel with the selftests
    // compiled OUT (the s42/INSTGUI and WXN-M3b failure). Default OFF => both selftests and their
    // call sites unlinked. Kept in sync with arroyo's mapping.
    if std::env::var("UNAOS_HCRONST").is_ok() { feats.push("hcronst"); }
    // PRTSCR-ST: UNAOS_PRTSCRST=1 arms the screen capture's BOOT-TIME-WRITE witness — see the knob's
    // note in arroyo. Mapped here as well as there, because a knob mapped in only one of the two
    // ships the feature disabled while the banner claims it is on (s42/INSTGUI, WXN-M3b).
    if std::env::var("UNAOS_PRTSCRST").is_ok() { feats.push("prtscrst"); }
    // XHCIKBD (B45): UNAOS_XHCIKBD=1 arms the x86 headless test's xHCI KEYBOARD leg — the burst scorer
    // in drivers/xhci/mod.rs (`xhcikbd_note`) — and, below, moves the one QEMU usb-kbd onto the xHCI
    // bus so the scorer has a device to score. A GATE knob, never a flight knob. Kept in sync with
    // arroyo's mapping (the KNOB→BUILDER WIRING CHECK reads this line).
    if std::env::var("UNAOS_XHCIKBD").is_ok() { feats.push("xhcikbd"); }
    // K-GPU: UNAOS_KEPLER=1 arms the GK107 (GT 650M) driver — probe/EVO-decode/PFIFO are further
    // gated by UNAOS_KEPLER_TAKEOVER / UNAOS_KEPLER_FIFO (option_env!, compile-time). Kept in sync
    // with arroyo's mapping. (The builder rebuilds the kernel, so this MUST be here or the feature
    // never reaches the kernel binary.)
    // BENCH-RIDE: read-only rMBP sitting ride-along probes (drivers/bench_ride.rs). Kept in sync
    // with arroyo's mapping. therm implies smc via the feature graph; all default OFF => unlinked.
    if std::env::var("UNAOS_THERM").is_ok() { feats.push("thermprobe"); }
    if std::env::var("UNAOS_PCILINK").is_ok() { feats.push("pcilink"); }
    if std::env::var("UNAOS_VROM").is_ok() { feats.push("vromprobe"); }
    if std::env::var("UNAOS_KEPLER").is_ok() { feats.push("nvidia-kepler"); }
    if std::env::var("UNAOS_KEPLER_TAKEOVER").is_ok() { feats.push("nvidia-kepler-takeover"); }
    if std::env::var("UNAOS_KEPLER_FIFO").is_ok() { feats.push("nvidia-kepler-fifo"); }
    // CE-LADDER: UNAOS_KEPLER_CE=1 arms the read-only copy-engine reconnaissance
    // (drivers/gpu/kepler_ce.rs). THIS list is what reaches the kernel binary for MEDIA
    // builds — a knob wired into arroyo alone would ship the ladder DISABLED while the
    // banner claims it is on (the s42/INSTGUI and WXN-M3b failure). Kept in sync with arroyo.
    if std::env::var("UNAOS_KEPLER_CE").is_ok() { feats.push("nvidia-kepler-ce"); }
    if std::env::var("UNAOS_KDISP_HOLD").is_ok() { feats.push("nvidia-kepler-kdisp-hold"); }
    // BEAMX86: UNAOS_BEAM=1 arms the x86 beam SOURCE — `kepler_display::beam_probe`, which samples
    // the live head's HEAD_STAT.VERT at the end of the Kepler takeover seam so `arch::scanout_beam()`
    // answers and `video::beam` stops folding every panel present to a no-op. It sits with the Kepler
    // family because that is what it needs to REACH its call site: UNAOS_KEPLER +
    // UNAOS_KEPLER_TAKEOVER, plus UNAOS_WC for the presents it brackets. DEFAULT OFF => the probe and
    // the arch arm are unlinked and `scanout_beam()` is a constant `None` => byte-identical media.
    // THIS list is what reaches the kernel binary for MEDIA builds, and the failure mode here is the
    // nastiest one in this file's collection: the metal witness `:: BEAMX86: … -> ARMED ::` would be
    // ABSENT FROM THE WIRE, which is indistinguishable from a head that never scanned, so a knob
    // wired into arroyo alone would not merely disable the fix — it would make the flight read as a
    // NEGATIVE RESULT about the hardware. Kept in sync with arroyo; `./arroyo check`'s KNOB→BUILDER
    // WIRING CHECK goes red if this line is dropped.
    if std::env::var("UNAOS_BEAM").is_ok() { feats.push("beam"); }
    // SHUTRESTORE (rmbp, 2026-09-15): the SEVEN restored R19 rungs — the refuted Kepler ladder steps
    // whose CODE had been deleted and is now back behind a knob apiece (RULINGS R19;
    // docs/dev/OS/08_VIDEO/SHUTOUT-REGISTER.md §7). THIS list is what reaches the kernel binary for
    // MEDIA builds, and the failure mode for a probe rung is the nastiest one in this file's
    // collection, the same one BEAMX86 names above: a rung armed in arroyo alone would print NO
    // witness token on the wire, and an ABSENT token is indistinguishable from a rung that ran and
    // found nothing — so the flight would read as a NEGATIVE RESULT ABOUT THE HARDWARE rather than
    // as a build that never carried the code (the s42/INSTGUI and rastmc failure, with a worse
    // ending). All seven DEFAULT OFF => unlinked and media unchanged. Kept in sync with arroyo;
    // `./arroyo check`'s KNOB→BUILDER WIRING CHECK goes red if any of these lines is dropped.
    if std::env::var("UNAOS_KEPLER_USERD_SNOOP").is_ok() { feats.push("nvidia-kepler-userdsnoop"); }
    if std::env::var("UNAOS_KEPLER_PFIFO_FLUSH").is_ok() { feats.push("nvidia-kepler-pfifoflush"); }
    if std::env::var("UNAOS_KEPLER_CTRL_ADDR").is_ok() { feats.push("nvidia-kepler-ctrladdr"); }
    if std::env::var("UNAOS_KEPLER_REPOINT").is_ok() { feats.push("nvidia-kepler-repoint"); }
    if std::env::var("UNAOS_KEPLER_LATCH_ARM").is_ok() { feats.push("nvidia-kepler-latcharm"); }
    if std::env::var("UNAOS_KEPLER_PITCH_LADDER").is_ok() { feats.push("nvidia-kepler-pitchladder"); }
    if std::env::var("UNAOS_KEPLER_GOP_OVERLAP").is_ok() { feats.push("nvidia-kepler-gopoverlap"); }
    // WC-X86: UNAOS_WC=1 arms the window compositor on the x86 panel path (video/desktop_uefi.rs) — activated
    // at the END of the Kepler takeover seam, after `fbcon::panel_console_resume`. x86_64-only
    // module; DEFAULT OFF => module + call site unlinked => byte-identical media. Needs
    // UNAOS_KEPLER + UNAOS_KEPLER_TAKEOVER to reach its seam. Kept in sync with arroyo's mapping.
    if std::env::var("UNAOS_WC").is_ok() { feats.push("wc"); }
    // PHASE31ROOT: UNAOS_BAR1EXP=<mode> selects a BAR1-wedge experiment arm
    // (docs/dev/OS/08_VIDEO/phase31-root.md). One mode exists: `uc` -> `bar1exp-uc`, retyping the
    // panel-aperture leaves UC (PAT PA3) instead of WC at `set_framebuffer_wc`. Memory TYPE only.
    // MEDIA builds re-derive the x86 feature set HERE, so the knob must be mapped here or a metal
    // boot ships the arm disabled while the banner claims it is armed (the s42/INSTGUI and rastmc
    // failure — this is the builder half of the two-place trap the arroyo CHECK now enforces).
    // Unknown modes are refused loudly, mirroring arroyo: an experiment knob that half-parses is
    // worse than one that stops the build. DEFAULT (unset) => feature unlinked => today's mapping.
    match std::env::var("UNAOS_BAR1EXP") {
        Ok(m) if m == "uc" => feats.push("bar1exp-uc"),
        Ok(m) => {
            eprintln!("UNAOS_BAR1EXP='{m}' is not a known mode (known: uc) — refusing to guess which experiment you meant.");
            std::process::exit(1);
        }
        Err(_) => {}
    }
    // PCIH: UNAOS_NOASPM=1 clears ASPM (LNKCTL[1:0]) on the Kepler link at init — the boot-8
    // endpoint-hang discriminator. DEFAULT OFF => feature unlinked => byte-identical media.
    // Kept in sync with arroyo's mapping.
    if std::env::var("UNAOS_NOASPM").is_ok() { feats.push("noaspm"); }
    // BAR1WEDGE: UNAOS_BAR1WEDGE=1 arms the FIRST-STALL register block at the tail of
    // drivers/gpu/pcihealth.rs — the falsifier that makes the never-flown UC arm (UNAOS_BAR1EXP=uc)
    // scorable. Root port only: a boot baseline + the completion-timeout decode, an arm-time W1C
    // clear of the three sticky latches, and one read-only `[pcih] wedge-sample` line per tripwire
    // crossing. THIS list is what reaches the kernel binary for MEDIA builds, and this knob's ENTIRE
    // product is witness lines — so a knob wired into arroyo alone would put `bar1wedge` in the
    // banner and not one `:: BAR1WEDGE:` string in the image, and the flight would read as a
    // negative result about the hardware rather than as a build that never carried the instrument
    // (BEAMX86's failure mode above, verbatim). Kept in sync with arroyo; `./arroyo check`'s
    // KNOB→BUILDER WIRING CHECK goes red if this line is dropped. DEFAULT OFF => feature unlinked
    // => byte-identical media.
    if std::env::var("UNAOS_BAR1WEDGE").is_ok() { feats.push("bar1wedge"); }
    // KFBIND (shut-out register §2 rung KF27): UNAOS_KEPLER_KFBIND=1 arms the PBDMA BASE DERIVATION
    // at drivers/gpu/kepler_fifo.rs — the PTOP device-info sweep, the DERIVED-vs-LEGACY base
    // comparison that audits the `0x40000 + i*0x2000` guess every PBDMA verdict since s#6 is read
    // through, and `IB_GET` (USERD +0x88) read as the falsifier for the first time in the campaign.
    // READ-ONLY: writes=0, restored=n/a, and the bind/enable write the rung is NAMED for prints as
    // `skipped write=... reason=uncited`. THIS list is what reaches the kernel binary for MEDIA
    // builds, and this knob's ENTIRE product is witness lines — so a knob wired into arroyo alone
    // would put `nvidia-kepler-kfbind` in the banner and not one `:: KFBIND:` string in the image,
    // and the flight would read as a negative result about the GK107 (`still-dark`) rather than as a
    // build that never carried the instrument (BEAMX86's failure mode, and BAR1WEDGE's above,
    // verbatim). Kept in sync with arroyo AND crates/kernel/Cargo.toml; `./arroyo check`'s
    // KNOB→BUILDER WIRING CHECK goes red if this line is dropped. DEFAULT OFF => module and both
    // call sites unlinked => byte-identical media.
    if std::env::var("UNAOS_KEPLER_KFBIND").is_ok() { feats.push("nvidia-kepler-kfbind"); }
    // KFCTXBIND (shut-out register §2 rung KF28): UNAOS_KEPLER_KFCTXBIND=1 arms the KF18/KF19
    // RE-RUN at drivers/gpu/kepler_fifo.rs `mod ctxbind` — the FECS ctx-ucode precondition CENSUSED
    // (ten §2 rows, every one with a sitting in its evidence column, and `0x409504` provably
    // unreachable by a `const _`), the channel's instance block read back through BAR1 with the
    // ADDRESS CLASS beside every word, the `chan_ctrl[i] fw-bound=… ours=…` sweep that would pin the
    // enable encoding BY DIFF, and IB_GET scored across the submit at the DERIVED base. The rung
    // gates itself on KF27's base ladder, re-evaluated read-only on the boot, and prints
    // `skipped reason=kf27-base-unresolved` when that ladder does not favour a derived base.
    // READ-ONLY: writes=0, restored=n/a, and the channel CTRL enable/bind the rung is NAMED for
    // prints `skipped write=chan_ctrl_enable reason=uncited`. THIS list is what reaches the kernel
    // binary for MEDIA builds, and this knob's ENTIRE product is witness lines — so a knob wired
    // into arroyo alone would put `nvidia-kepler-kfctxbind` in the banner and not one
    // `:: KFCTXBIND:` string in the image, and the flight would read as an ELEVENTH ELIMINATION
    // about the GK107 rather than as a build that never carried the instrument (BEAMX86's failure
    // mode, and BAR1WEDGE's and KFBIND's above, verbatim). Kept in sync with arroyo AND
    // crates/kernel/Cargo.toml; `./arroyo check`'s KNOB→BUILDER WIRING CHECK goes red if this line
    // is dropped. DEFAULT OFF => mod ctxbind and both call sites unlinked => byte-identical media.
    if std::env::var("UNAOS_KEPLER_KFCTXBIND").is_ok() { feats.push("nvidia-kepler-kfctxbind"); }
    // KDHEAD (shut-out register §1 rung KD14): UNAOS_KEPLER_KDHEAD=1 arms the BRACKETED PER-HEAD
    // DECODE at the tail of drivers/gpu/kepler_display.rs — KD3's per-head EVO/CRTC decode re-run
    // with KD4's HEAD_STAT reading as the control bracket (borrowed from BEAMX86's one per-head
    // census, never re-sampled) and the per-head stride MEASURED across five candidate blocks
    // instead of assumed, with a two-pass filter so a counter cannot make a collapsed stride look
    // like four distinct heads. READ-ONLY: writes=0. THIS list is what reaches the kernel binary for
    // MEDIA builds, and this knob's ENTIRE product is witness lines — so a knob wired into arroyo
    // alone would put `nvidia-kepler-kdhead` in the banner and not one `:: KDHEAD:` string in the
    // image, and a flight with no `heads_distinct=` line would read as "the per-head blocks are
    // dead", which is KD3's own shut-out verdict reached a second time from a build defect, on the
    // one rung whose purpose is to correct it (BEAMX86's failure mode, and BAR1WEDGE's, KFBIND's and
    // KFCTXBIND's above, verbatim). Kept in sync with arroyo AND crates/kernel/Cargo.toml;
    // `./arroyo check`'s KNOB→BUILDER WIRING CHECK goes red if this line is dropped. DEFAULT OFF =>
    // every item, the call site and the census publication inside beam_probe are unlinked =>
    // byte-identical media.
    if std::env::var("UNAOS_KEPLER_KDHEAD").is_ok() { feats.push("nvidia-kepler-kdhead"); }
    // CTRLBIND (shut-out register §2 rung KF9b): UNAOS_KEPLER_CTRLBIND=1 arms the PER-TARGET CHANNEL
    // BRINGUP RE-RUN inside the restored KF9 CTRL_ADDR audit in drivers/gpu/kepler.rs — the half of
    // KF9 the 2026-09-15 restore could not carry, because the s13 original re-ran the whole channel
    // bringup inside its target loop and that apparatus was deleted with it. THIS list is what
    // reaches the kernel binary for MEDIA builds, and this knob's ENTIRE product is witness lines —
    // so a knob wired into arroyo alone would put `nvidia-kepler-ctrlbind` in the banner and not one
    // `:: kepler: ctrlbind ` string in the image, and the flight would read as "no TARGET encoding
    // changes the strip" — a THIRTEENTH ELIMINATION about the GK107 — rather than as a build that
    // never carried the instrument (BEAMX86's failure mode, and KFBIND's and KDHEAD's above,
    // verbatim). Kept in sync with arroyo AND crates/kernel/Cargo.toml; `./arroyo check`'s
    // KNOB→BUILDER WIRING CHECK goes red if this line is dropped. DEFAULT OFF => every call site and
    // every `:: kepler: ctrlbind ` string unlinked => byte-identical media.
    if std::env::var("UNAOS_KEPLER_CTRLBIND").is_ok() { feats.push("nvidia-kepler-ctrlbind"); }
    // KFUNWEDGE (shut-out register §2 rung KF29): UNAOS_KEPLER_KFUNWEDGE=1 arms THE UN-WEDGE
    // EXPERIMENT at drivers/gpu/kepler_fifo.rs `mod unwedge` — a baseline of the cited PRING fault
    // words (PBUS_INTR 0x1100 and the PIBUS trio), then EXACTLY ONE deliberate host read of
    // 0x409504 (the offset falcon_microcode_spec.md §5.4 is named for), then an observe pass with
    // NV_PMC_BOOT_0 as the out-of-unit control, then a W1C of exactly the bits read as SET, then a
    // cpuctl re-read scored UNWEDGED / STILL-POISONED / POISON-CONFINED / NOT-POISONED / VOID-*.
    // ⚠⚠ THIS KNOB SHIPS A SACRIFICIAL BOOT: the Kepler drives the rMBP panel, the operator may
    // lose the display until a POWER CYCLE, and a poison is NOT restorable (`restored=IMPOSSIBLE`).
    // Fly it LAST and ALONE — never with BEAMX86, KDHEAD, KFBIND or KFCTXBIND aboard.
    // THIS list is what reaches the kernel binary for MEDIA builds, and this knob is the worst case
    // the KNOB→BUILDER WIRING CHECK exists for: a knob wired into arroyo alone would put
    // `nvidia-kepler-kfunwedge` in the banner and not one `:: KFUNWEDGE:` string in the image — and
    // the operator would have spent a sacrificial boot, and possibly the panel, on a build that
    // never carried the rung (BEAMX86's failure mode, and BAR1WEDGE's, KFBIND's and KFCTXBIND's
    // above, at its most expensive). Kept in sync with arroyo AND crates/kernel/Cargo.toml;
    // `./arroyo check`'s KNOB→BUILDER WIRING CHECK goes red if this line is dropped. DEFAULT OFF =>
    // mod unwedge, the call site and kepler.rs's `fecs_poison_ledger` accessor unlinked =>
    // byte-identical media.
    if std::env::var("UNAOS_KEPLER_KFUNWEDGE").is_ok() { feats.push("nvidia-kepler-kfunwedge"); }
    // KVBLANK (rmbp, the GPU line under R53, rungs kvblank-measure + kvblank-wait):
    // UNAOS_KEPLER_VBLANK=1 arms the Kepler head's VBLANK EDGE — counted, timed and phased off
    // `HEAD_STAT.VERT[31:16]` out of the word `scanout_beam` was already reading — and the wait
    // `video/beam.rs::hold` takes instead of spinning on the raster position. B135 §7 (VUGPERF)
    // measured the spin at `[wc-h] win=8 beamwaits=4336` with `beamwait_us` summing to 10 766 899,
    // a 2.48 ms mean hold against a 16.667 ms frame.
    // THIS list is what reaches the kernel binary for MEDIA builds AND for the QEMU gate, and this
    // knob's entire product is witness lines — so a knob wired into arroyo alone puts
    // `nvidia-kepler-vblank` in the banner and not one `:: kepler: vblank ` string in the image.
    // ⚠ THAT IS NOT HYPOTHETICAL HERE: it was MEASURED on this rung's first armed gate run, before
    // this line existed — banner named the feature, `awk 'index($0,":: kepler: vblank")'` over
    // target/serial.log returned ZERO lines. BEAMX86's failure mode, and BAR1WEDGE's, KFBIND's,
    // KFCTXBIND's, KDHEAD's and CTRLBIND's above, verbatim, caught by the gate rather than by a
    // card. Kept in sync with arroyo AND crates/kernel/Cargo.toml; `./arroyo check`'s KNOB→BUILDER
    // WIRING CHECK goes red if this line is dropped. DEFAULT OFF => the module, all four call sites
    // and every `:: kepler: vblank ` string unlinked => byte-identical media.
    if std::env::var("UNAOS_KEPLER_VBLANK").is_ok() { feats.push("nvidia-kepler-vblank"); }
    // R0 / RTWIT: UNAOS_RTWIT=1 arms the WORST-CASE RULER (`rtwit`) — the `[rtwit]` tail instruments
    // (input→present latency, per-lock max hold, max interrupt-mask span). MAXes only; pure measurement,
    // no scheduling/locking/present change. x86_64-only in effect; DEFAULT OFF => empty inline shims,
    // byte-inert. This list reaches the KERNEL build for MEDIA, so a metal boot can arm the ruler.
    // Kept in sync with arroyo's mapping.
    if std::env::var("UNAOS_RTWIT").is_ok() { feats.push("rtwit"); }
    // DEADMAN: UNAOS_DEADMAN=1 arms the timer-ISR witness that survives a wedged render-service
    // pass — one unconditional `[deadman]` line per second, so silence is distinguishable from
    // idleness. Kept in sync with arroyo.
    if std::env::var("UNAOS_DEADMAN").is_ok() { feats.push("deadman"); }
    // WEDGEINJ: UNAOS_WEDGEINJ=1 arms the injected phase-33 park — at 30 s the published render core
    // clears IF and spins forever from inside the present blit, reproducing the metal wedge so
    // WCSER-STEAL and WCSER-REHOME can be gated by execution instead of by compilation. TEST-ONLY;
    // costs one AP for the rest of the run. Never arm on bench media. Kept in sync with arroyo.
    if std::env::var("UNAOS_WEDGEINJ").is_ok() { feats.push("wedgeinj"); }
    // R1 / RTPI: UNAOS_RTPI=1 arms PRIORITY INHERITANCE on the x86 sleeping `Mutex` plus its `[rtpi]`
    // witness. Unlike RTWIT, this CHANGES scheduling — the holder of a contended `Mutex` inherits a
    // blocked higher-priority task's priority (transitively) until release. x86_64-only in effect;
    // DEFAULT OFF => PI fields absent, original `Mutex::lock` path, inline-shim witness => byte-identical
    // media. This list reaches the KERNEL build for MEDIA, so a metal boot can arm it. Sync with arroyo.
    if std::env::var("UNAOS_RTPI").is_ok() { feats.push("rtpi"); }
    // VSYNC-PACE r3: UNAOS_VSYNCPACE=1 ARMS the kernel-side present pacer. ⚠ POLARITY INVERTED from
    // GR22's UNAOS_NOPACE — the pacer is now DEFAULT OFF under `wc`, so an unmodified metal boot presents
    // UNRESTRICTED and this knob is the opt-in that restores the vsync-cadence path. This list is what
    // reaches the KERNEL build for MEDIA, so the knob has to be mapped here as well as in arroyo, or a
    // metal boot could never be paced at all. Kept in sync with arroyo's mapping.
    if std::env::var("UNAOS_VSYNCPACE").is_ok() { feats.push("vsyncpace"); }
    // INSTGUI: UNAOS_INSTGUI=1 opens the graphical installer dialog on the wc desktop. The cargo
    // feature implies `wc` + `installdemo`, but this list is what reaches the KERNEL build for
    // media, so the knob must be mapped here too (arroyo's own list only covers non-media paths —
    // that asymmetry is why s42 shipped without the dialog).
    if std::env::var("UNAOS_INSTGUI").is_ok() { feats.push("instgui"); }
    // WEDGE-2: UNAOS_WEDGE2=1 arms the `wedge2` feature — raw-UART `<F1>`..`<F9>` last-words
    // breadcrumbs along the focus-raise/composite chain (x86: bare 16550 at 0x3F8, no lock). Media
    // builds come from THIS list, not arroyo's (the s42/INSTGUI lesson), so the knob is mapped here
    // too. Default off => call sites vanish => no `<F` token in the image (strings-verifiable).
    if std::env::var("UNAOS_WEDGE2").is_ok() { feats.push("wedge2"); }
    // IVB-iGPU: UNAOS_IVB=1 arms the Intel HD 4000 ground-truth probe (sitting #6). Kept in sync
    // with arroyo's mapping — boot-1 of sitting #6 shipped WITHOUT this line and carried no probe.
    // unaos_ivb rides the same knob: it adds the teardown-trace fields to the SHARED BootInfo
    // struct, so kernel and bootloader must agree on it (see the bootloader build below).
    if std::env::var("UNAOS_IVB").is_ok() { feats.push("intel-ivb"); feats.push("unaos_ivb"); }
    // GEN7-3D: UNAOS_IVB3D=1 arms the Ivy Bridge render-engine reconnaissance rung R1
    // (drivers/gpu/gen7.rs) — READ-ONLY, zero MMIO/config writes, no display register.
    // Kept in sync with arroyo's mapping: the builder REBUILDS the kernel for media, so a
    // knob armed only in arroyo ships the rung absent while the banner says otherwise —
    // the s42/INSTGUI failure, and the reason this leg is not optional.
    if std::env::var("UNAOS_IVB3D").is_ok() { feats.push("gen7"); }
    // GEN7-3D R8: UNAOS_IVB3D_R8=1 arms the framebuffer-geometry BCS blit — the rung after R7's
    // metal-verified 16x16 blit (drivers/gpu/gen7.rs, `mod r8`). It runs only on a boot whose R7
    // verdict is `r7-blit-verified`, writes nothing into the panel (the destination is CPU-readable
    // scratch at the framebuffer's pitch; the Kepler owns the panel), and captures/restores/re-reads
    // every register it touches. `gen7` is pushed alongside because the banner and the artifact must
    // agree — cargo would resolve the implication anyway, but the `⚡ kernel features:` line is built
    // from THIS list, and a banner that omits `gen7` is the BANNERCERT lie. Kept in sync with
    // arroyo's mapping: the builder REBUILDS the kernel for media, so a knob armed only in arroyo
    // ships the rung absent while the banner says otherwise — the s42/INSTGUI failure.
    if std::env::var("UNAOS_IVB3D_R8").is_ok() { feats.push("gen7"); feats.push("gen7r8"); }
    // GMUX-IGD: UNAOS_GMUX_IGD=1 arms the display-mux switch to the integrated GPU with an
    // unwind-stack restore on the same call stack (round 13 removed the timed auto-revert).
    // Kept in sync with arroyo's mapping — the builder rebuilds the kernel, so a knob
    // armed in arroyo but missing HERE ships the feature DISABLED while every log line claims it is
    // on. That failure mode has already cost this project weeks on the kepler and igpu lanes.
    if std::env::var("UNAOS_GMUX_IGD").is_ok() { feats.push("gmux_igd"); }
    // INSTALL-CORE: UNAOS_INSTALLDEMO=1 arms the installer engine + its x86 boot witness (GPT writer
    // + FAT32 formatter + extent content-verify) against the blank scratch disk attached below under
    // the same knob. Kept in sync with arroyo's mapping. (The builder rebuilds the kernel, so this
    // MUST be here or the feature never reaches the kernel binary.)
    if std::env::var("UNAOS_INSTALLDEMO").is_ok() { feats.push("installdemo"); }
    // VPERF M2: the fbcon viewport-cap bench lever (implies videobench). x86_64 only.
    if std::env::var("UNAOS_VIDEOCAP").is_ok() { feats.push("videocap"); }
    // SMOLNET-DEFAULT: smoltcp is the DEFAULT x86 net stack (2026-07-17). Push `smolnet` (shell
    // ping/arp/ifconfig + socket syscalls + boot witnesses) UNLESS opted out with UNAOS_NOSMOLNET=1,
    // which drops the feature => the hand-rolled `net` crate is the whole net path, byte-identical to
    // the pre-flip default (PORTSW-1/EHCI-4 default-ON/negative-knob policy). x86-only optional dep +
    // module. (The builder rebuilds the kernel, so this MUST mirror arroyo's mapping.)
    if std::env::var("UNAOS_NOSMOLNET").is_err() { feats.push("smolnet"); }
    // `tegra` (Jetson Orin / Tegra234 UART) is an aarch64 board feature; mapped here for parity with
    // the `pi` knob, though this x86_64 builder never produces aarch64 media (the `arroyo` script does).
    if std::env::var("UNAOS_TEGRA").is_ok() { feats.push("tegra"); }
    // ORIN-SMP-2: UNAOS_SMPPROBE=<n> arms the JM5 CPU_ON firmware-wall investigation probe (tegra-only
    // aarch64; the numeric value selects the experiment via option_env). Mapped here for parity with
    // arroyo's feature list, though this x86_64 builder never produces the aarch64 tegra media.
    if std::env::var("UNAOS_SMPPROBE").is_ok() { feats.push("smpprobe"); }
    // SMPMARK (ORIN-SMP-3): UNAOS_SMPMARK=1 arms the three secondary-bring-up marks in
    // arch/aarch64/smp_virt.rs (`:P:` / `:R<idx>:` / `:A:`). aarch64-only in effect; mapped here for
    // parity with arroyo's feature list, though this x86_64 builder never produces aarch64 media.
    if std::env::var("UNAOS_SMPMARK").is_ok() { feats.push("smpmark"); }
    // ORIN-SMP-DEFAULT: the real 6-core Orin SMP kick-off is DEFAULT-ON for tegra builds (opt out with
    // UNAOS_NOTEGRASMP=1). `tegrasmp` implies the aarch64 `tegra` board feature; this x86_64 builder
    // never produces aarch64 tegra media (arroyo's esp-jetson does, where the default-on lives), so this
    // maps the EXPLICIT UNAOS_TEGRASMP=1 knob for parity only. UNAOS_NOTEGRASMP is a no-op here (nothing
    // to suppress on x86 media). Kept in sync with arroyo's mapping.
    if std::env::var("UNAOS_TEGRASMP").is_ok() { feats.push("tegrasmp"); }
    // FTDIRX (rmbp A9 / LEDGER S29, x86 half): UNAOS_FTDIRX=1 gives the FTDI console its RECEIVE
    // half — one Normal TRB outstanding on the FT232's bulk-IN 0x81, the two modem-status bytes
    // stripped, the rest pushed as `pal::Event::Key`. MUST be listed here and not only in `arroyo`:
    // THIS list is the one the x86 kernel that actually boots is built from, and a knob mapped there
    // alone is the rastmc failure — the `⚡ kernel features:` banner says the feature is on and the
    // image carries none of it, which for an RX arc reads exactly like "the cable received nothing".
    // Needs UNAOS_USBSERIAL=1 to have a cable at all, and UNAOS_FTDIRX_INJECT=<path> (below) to have
    // anything to receive under QEMU. Kept in sync with arroyo's mapping.
    if std::env::var("UNAOS_FTDIRX").is_ok() { feats.push("ftdirx"); }
    // UVC (CAMERA1, rmbp-ledger B143): UNAOS_UVC=1 arms the USB Video Class census and the
    // VS_PROBE_CONTROL negotiation on the rMBP's built-in FaceTime HD camera (05ac:8510, class
    // 0xEF with a video IAD, EHCI controller [0] addr 2). MUST be listed here and not only in
    // `arroyo`, for the reason the FTDIRX line above states and with the same sting: THIS list is
    // what the x86 kernel that actually boots is built from — for `esp-x86`/`vm-image` AND for
    // `./arroyo test`, whose QEMU this builder owns — so a knob mapped there alone would light the
    // `⚡ kernel features:` banner on an image carrying not one `[uvc]` string. MEASURED before
    // this line existed, and it is the sdwrite/rastmc shape caught by the gate built for it:
    // banner-cert on the knob-off artifact with `uvc` on the banner reads
    // `feature=uvc witness=[uvc] commit=withheld hits=0 -> MISSING`, exit 1. Implies `ehcihid` in
    // Cargo.toml, which this list already pushes by default, so pushing `uvc` alone is the whole
    // delta. Default OFF => the feature is absent and the image is byte-identical
    // (`./arroyo knoboff uvc`). Kept in sync with arroyo's mapping.
    if std::env::var("UNAOS_UVC").is_ok() { feats.push("uvc"); }
    // IOAPIC (rmbp-ledger B147): UNAOS_IOAPIC=1 arms `arch/x86_64/ioapic.rs` — the Intel 82093AA
    // redirection table, so a PCI function with no usable MSI capability can have its INTx routed
    // to a vector instead of staying POLLED (rmbp flight 11: `ISRARM REFUSED … no IOAPIC`).
    // MUST be listed here and not only in `arroyo`, and this knob is the worst case for that rule:
    // THIS list is what compiles the kernel `./arroyo test` boots and the kernel the rMBP bench
    // boots, so a mapping in arroyo alone would put `ioapic` on the banner while the image carried
    // no redirection-table writer — and the boot would print the exact refusal this arc exists to
    // end, with the banner claiming the fix was in. `scripts/banner-cert.sh` now reads the artifact
    // for `[ioapic] census ioapics=` whenever the banner names the feature, which is the same
    // question asked from the other side. Kept in sync with arroyo's mapping and Cargo.toml.
    if std::env::var("UNAOS_IOAPIC").is_ok() { feats.push("ioapic"); }
    if !feats.is_empty() {
        let list = feats.join(",");
        kernel_cmd.arg("--features").arg(&list);
        println!("   kernel features: {list}");
    }
    let kernel_status = kernel_cmd.status().unwrap();

    if !kernel_status.success() {
        panic!("Kernel build failed");
    }

    println!("🔹 Building x86_64 UEFI Bootloader...");
    let mut bootloader_cmd = Command::new("cargo");
    bootloader_cmd
        .current_dir(workspace_dir.join("crates/bootloader"))
        .arg("+nightly")
        .arg("build")
        .arg("--release")
        .arg("--target").arg("x86_64-unknown-uefi")
        .arg("-Z").arg("build-std=core,compiler_builtins,alloc")
        .arg("-Z").arg("build-std-features=compiler-builtins-mem");
    // BootInfo ABI: unaos_ivb adds fields to the shared BootInfo struct — the bootloader must
    // arm it from the SAME knob as the kernel above or the two binaries disagree on the layout.
    if std::env::var("UNAOS_IVB").is_ok() {
        bootloader_cmd.arg("--features").arg("unaos_ivb");
        println!("   bootloader features: unaos_ivb");
    }
    let bootloader_status = bootloader_cmd.status().unwrap();

    if !bootloader_status.success() {
        panic!("Bootloader build failed");
    }

    let kernel_bin = target_dir.join("x86_64-unaos/release/unaos-kernel");
    let bootloader_bin = target_dir.join("x86_64-unknown-uefi/release/bootloader.efi");

    println!("🔹 Packaging ESP (EFI System Partition)...");
    let _ = std::fs::remove_dir_all(&esp_dir);
    let boot_dir = esp_dir.join("EFI/BOOT");
    std::fs::create_dir_all(&boot_dir).unwrap();
    // LAYOUT (orin 18): THE LAUNCHABLE PROGRAMS GO IN `APPS/`. `fat::APPS_DIR` is the on-medium
    // spelling and `shell::EXEC_ROOT` (`/apps`) is the namespace one; WINX-2, WINX-8, PULSE-W and
    // the desktop app launcher read through `FatFs::find_app`, which looks there.
    //
    // Two groups stay in the ROOT. The firmware's own files — EFI/BOOT/BOOTX64.EFI and kernel.elf —
    // because UEFI reads those by fixed path. And HELLO.BIN, because the U2 program is ALSO opened
    // by EL0, by name, through `sys_open`, whose namespace is a flat 8.3 volume root with no
    // directory component at all — a syscall-ABI fact, not a layout preference.
    let esp_apps = esp_dir.join("APPS");
    std::fs::create_dir_all(&esp_apps).unwrap();
    
    std::fs::copy(&bootloader_bin, boot_dir.join("BOOTX64.EFI")).unwrap();
    std::fs::copy(&kernel_bin, esp_dir.join("kernel.elf")).unwrap();

    // A small text file so the in-kernel FAT reader has something to `cat` on the real boot stick:
    // after boot, `ls` shows EFI/ + kernel.elf + hello.txt and `cat hello.txt` reads it back off the
    // FAT32 volume — proving USB mass-storage block I/O + FAT parsing on metal.
    std::fs::write(
        esp_dir.join("hello.txt"),
        "Hello from UnaOS on real hardware!\nThis file was read off the FAT32 boot stick by the in-kernel FAT reader.\n",
    ).unwrap();

    // U2: the x86 ring-3 "hello from disk" program (crates/user-blob-x86, built by arroyo's
    // build_user_hello_x86 to target/hello.bin). Copy it onto the ESP as HELLO.BIN so the metal boot
    // media carries it; the kernel's U2 FAT loader reads it off the volume and runs it in ring 3.
    // (make-fat-img.sh copies the same target/hello.bin onto the QEMU FAT stick images.) Absent when
    // the blob wasn't built (a bare `cargo run` in builder/) — then U2 simply NoFile-skips, harmless.
    let hello_bin = target_dir.join("hello.bin");
    if hello_bin.exists() {
        std::fs::copy(&hello_bin, esp_dir.join("HELLO.BIN")).unwrap();
        println!("   U2: copied HELLO.BIN onto the ESP root (EL0 opens it by name; see APPS/ note)");
    } else {
        println!("   U2: target/hello.bin absent — ESP has no HELLO.BIN (run via ./arroyo esp-x86)");
    }

    // WINX-5: the x86 EL0 persistence program (crates/user-stat, built by arroyo's build_user_stat_x86
    // to target/STAT-X86.ELF). Copy it onto the ESP as STAT.ELF so the metal boot media carries it, the
    // same way HELLO.BIN reaches the volume just above; the x86 shell's `run`/`bg` read the FAT boot
    // partition's root, so `bg /apps/STAT.ELF` finds it there. The name is un-suffixed ON the volume
    // (STAT.ELF, not STAT-X86.ELF) because the operator command should read the same on both arches —
    // the arch suffix exists only in target/, where both arches' images share one directory.
    // Absent when the program wasn't built (a bare `cargo run` in builder/) — then `run`/`bg` simply
    // report -ENOENT, harmless.
    let stat_elf = target_dir.join("STAT-X86.ELF");
    if stat_elf.exists() {
        std::fs::copy(&stat_elf, esp_apps.join("STAT.ELF")).unwrap();
        println!("   WINX: copied STAT.ELF into APPS/ on the ESP (bg /apps/STAT.ELF)");
    } else {
        println!("   WINX: target/STAT-X86.ELF absent — ESP has no STAT.ELF (run via ./arroyo esp-x86)");
    }

    // WINX-7: the x86 EL0 mini-vug (crates/user-vug, built by arroyo's build_user_vug_x86 to
    // target/VUG-X86.ELF), staged as VUG.ELF exactly like STAT.ELF above and for the same reasons —
    // un-suffixed on the volume so `bg /apps/VUG.ELF` reads the same on both arches.
    //
    // VUGSCENE: THREE images, not one. `crates/user-vug` is built three times from the same source —
    // adaptive (VUG.ELF), pinned to the classic wireframe (VUGC.ELF) and pinned to the full shard
    // (VUGX.ELF) — because `bg`/`run` carry a path and no argv, so a benchmarking pin has nowhere else to
    // live. All three names are 8.3-clean and go in the FAT root beside STAT.ELF/PULSE.ELF. Absent images
    // are skipped exactly as the single one always was.
    for (src, dst) in [
        ("VUG-X86.ELF", "VUG.ELF"),
        ("VUGC-X86.ELF", "VUGC.ELF"),
        ("VUGX-X86.ELF", "VUGX.ELF"),
        // KVUG: the fourth image — the IN-KERNEL vug (crates/kernel/src/vug.rs) carried into EL0, whose
        // `m` key cycles its three historical screens. Same reasoning as the pins: no argv, so the only
        // channel a mode set can travel down is a distinct image with a distinct 8.3 name.
        ("VUGK-X86.ELF", "VUGK.ELF"),
    ] {
        let vug_elf = target_dir.join(src);
        if vug_elf.exists() {
            std::fs::copy(&vug_elf, esp_apps.join(dst)).unwrap();
            println!("   WINX: copied {dst} into APPS/ on the ESP (bg /apps/{dst})");
        } else {
            println!("   WINX: target/{src} absent — ESP has no {dst} (run via ./arroyo esp-x86)");
        }
    }

    // PULSE-1: the x86 EL0 cpu-pulse monitor (crates/user-pulse, built by arroyo's build_user_pulse_x86 to
    // target/PULSE-X86.ELF), staged as PULSE.ELF exactly like STAT.ELF/VUG.ELF above and for the same
    // reasons — un-suffixed on the volume so `bg /apps/PULSE.ELF` reads the same on both arches.
    let pulse_elf = target_dir.join("PULSE-X86.ELF");
    if pulse_elf.exists() {
        std::fs::copy(&pulse_elf, esp_apps.join("PULSE.ELF")).unwrap();
        println!("   PULSE: copied PULSE.ELF into APPS/ on the ESP (bg /apps/PULSE.ELF)");
    } else {
        println!("   PULSE: target/PULSE-X86.ELF absent — ESP has no PULSE.ELF (run via ./arroyo esp-x86)");
    }

    // -----------------------------------------------------------------------------------------
    // WINX-7 PKG — the DATA tree: the EL0 artifacts staged for the volume the RUNNING KERNEL reads.
    //
    // THE DEFECT THIS FIXES, from an attended rMBP boot:
    //     :: WINX-2: STAT.ELF absent from the boot volume — end-to-end witness skipped ::
    // …with STAT.ELF verifiably present and byte-correct on the card that was booted. The FAT mount
    // SUCCEEDED; `find_in_root` is what came back empty. The reason is that on x86 there are TWO
    // volumes and the build only ever staged ONE of them:
    //
    //   * UEFI boots the ESP — whichever volume the firmware picked, on the rMBP typically the SD
    //     card. `crates/bootloader` reads `kernel.elf` off it through firmware boot services and then
    //     ExitBootServices, after which that volume is unreachable forever.
    //   * The KERNEL's `fs::fat::mount()` binds the global `drivers::block::BLOCK_DEVICE`, and on x86
    //     the ONLY writer of that global is the xHCI mass-storage bring-up — i.e. the USB stick on
    //     `storage_slot`. There is no SD, AHCI, SATA or NVMe driver on this arch, and `BootInfo`
    //     carries no boot-device handle, so the kernel cannot learn what it booted from, let alone
    //     read it.
    //
    // So `bg /apps/STAT.ELF` searches the USB stick while the build put STAT.ELF on the ESP. When the
    // operator boots a SINGLE stick that is both, the two coincide and everything works — which is
    // precisely why this went unnoticed: the `esp-x86` procedure assumed one stick, and the bench has
    // two devices. The kernel-side message even calls the mounted volume "the boot partition", which
    // it has no way to verify and which was, here, false.
    //
    // THE FIX is to stage the runtime artifacts into their own tree, named for what it actually is —
    // the DATA volume the kernel drives — so the operator has something unambiguous to write onto the
    // USB stick. The ESP copies above are KEPT, deliberately: they are correct and sufficient for a
    // single-stick boot, and removing them would break that (working) configuration to fix a
    // two-device one. What changes is that the two-device configuration is now expressible at all.
    //
    // The tree carries ONLY what the kernel reads at run time — no EFI/, no kernel.elf, no bootloader.
    // Those belong to the firmware's volume and would be dead weight (and a confusing second bootable
    // -looking volume) on the data stick.
    let data_dir = target_dir.join("x86_64_data");
    let _ = std::fs::remove_dir_all(&data_dir);
    std::fs::create_dir_all(&data_dir).unwrap();
    // LAYOUT (orin 18): the DATA volume gets the same `APPS/` directory the ESP just got — it is
    // the volume the RUNNING kernel reads, so it is the one whose layout the loader is looking at.
    let data_apps = data_dir.join("APPS");
    std::fs::create_dir_all(&data_apps).unwrap();
    let mut staged_data: Vec<&str> = Vec::new();
    // HELLO.BIN first, and into the volume ROOT — see the ESP note above (EL0 names it directly).
    if target_dir.join("hello.bin").exists() {
        std::fs::copy(target_dir.join("hello.bin"), data_dir.join("HELLO.BIN")).unwrap();
        staged_data.push("HELLO.BIN (root)");
    }
    for (src, dst) in [
        (target_dir.join("STAT-X86.ELF"), "STAT.ELF"),
        (target_dir.join("VUG-X86.ELF"), "VUG.ELF"),
        // VUGSCENE: the two PINNED vug images ride the data volume too — the pin exists for benchmarking,
        // and a benchmark that cannot be launched from the volume the kernel actually reads is no pin.
        (target_dir.join("VUGC-X86.ELF"), "VUGC.ELF"),
        (target_dir.join("VUGX-X86.ELF"), "VUGX.ELF"),
        // KVUG: the kernel-vug image rides the data volume for the same reason the pins do — `bg
        // /apps/VUGK.ELF` must reach the volume the kernel actually reads.
        (target_dir.join("VUGK-X86.ELF"), "VUGK.ELF"),
        (target_dir.join("PULSE-X86.ELF"), "PULSE.ELF"),
    ] {
        if src.exists() {
            std::fs::copy(&src, data_apps.join(dst)).unwrap();
            staged_data.push(dst);
        }
    }
    // `hello.txt` rides along so the operator has a trivial `cat hello.txt` probe that proves the
    // kernel is reading THIS volume — the one-command answer to "did I write the right stick?".
    std::fs::write(
        data_dir.join("hello.txt"),
        "Hello from UnaOS on real hardware!\nThis file was read off the FAT32 DATA volume by the in-kernel FAT reader.\n",
    ).unwrap();

    // STOR-1 fixtures: scripts/make-fat-img.sh plants these for the QEMU single-stick FAT image, and
    // the STOR-1 storage witnesses (arch/x86_64/syscall.rs) are calibrated against their exact bytes.
    // On metal, the kernel's fs::fat::mount() binds the USB stick — i.e. THIS data volume, not the ESP
    // — so a witness that reads a "non-staged on-disk file" needs that file physically here too, or it
    // fails for a media reason (fixture absent) rather than a code reason. Byte-identical to the QEMU
    // plant; see make-fat-img.sh's stage_contents() for the specification these mirror.

    // STOR-1 S7: README.TXT, read dynamically off the pre-stage set. The witness (s7_openany_witness)
    // only checks the file begins with this 16-byte PREFIX and is >= 16 bytes, so the exact trailing
    // text is not witness-critical — kept byte-identical to make-fat-img.sh's `part` layout anyway.
    std::fs::write(
        data_dir.join("readme.txt"),
        "UnaOS read-only FAT32/16 reader test volume (part layout).\n",
    ).unwrap();

    // U9x M2: SCRATCH.BIN — 1024 bytes of 0xEE (U9X_SCRATCH_FILL). Without this on-disk, the U9x fixture
    // still passes in its M1 in-memory-only mode (SCRATCH_CLUSTER stays 0), but silently skips the M2
    // disk-write-back proof — planting it here exercises the real path on metal instead of the fallback.
    std::fs::write(data_dir.join("SCRATCH.BIN"), vec![0xEEu8; 1024]).unwrap();

    // U10 GROW: GROW.BIN — 512 bytes of 0xC1 (U10_GROW_FILLER), exactly one 512-byte cluster. Same M1
    // fallback caveat as SCRATCH.BIN above (GROW_CLUSTER stays 0 without a real on-disk file).
    std::fs::write(data_dir.join("GROW.BIN"), vec![0xC1u8; 512]).unwrap();

    // STOR-1 S8: S8W.BIN — 64 bytes of 0xA5 (the s8_write_witness SEED). NEVER a staged name
    // (HELLO/SCRATCH/GROW.BIN) and NEVER README.TXT (S7 checks that file's prefix) — a dedicated
    // dynamic-open RW target the witness overwrites in place, reads back, then restores, so the file
    // stays pristine and idempotent across boots.
    std::fs::write(data_dir.join("S8W.BIN"), vec![0xA5u8; 64]).unwrap();

    // SINKHOLE-1/ZEOLITE-2: BLOCK.TXT — the DNS resolver's hosts-format blocklist, read via the same S7
    // dynamic-open path. Absent, the resolver falls back to its compiled-in builtin list (not a hard
    // fail) — planted here so metal exercises the real on-disk parse instead of the fallback. Byte-
    // identical to make-fat-img.sh's heredoc.
    std::fs::write(
        data_dir.join("BLOCK.TXT"),
        "# zeolite DNS sinkhole blocklist (hosts format)\n\
         0.0.0.0 ads.example\n\
         0.0.0.0 track.example   # inline comment tolerated\n\
         \n\
         ; semicolon comments and blank lines are skipped\n\
         127.0.0.1 telemetry.example\n",
    ).unwrap();

    println!(
        "   WINX-7 PKG: data volume tree target/x86_64_data/ — {} (all but HELLO.BIN under APPS/; + hello.txt, readme.txt, SCRATCH.BIN, GROW.BIN, S8W.BIN, BLOCK.TXT in the root)",
        if staged_data.is_empty() { "no EL0 artifacts built".to_string() } else { staged_data.join(", ") }
    );

    // VMIMAGE-1: package the just-built ESP tree into ONE self-contained GPT+FAT32 disk image
    // (target/vm/unaos-x86-<git7>.img) and stop — no QEMU. Reuses the SAME build products packed
    // above; the image builder never rebuilds. UNAOS_VM_GIT7 carries the short git hash (identity
    // + deterministic-GUID seed); arroyo's `vm-image` sets it. See builder/src/vm_image.rs.
    if std::env::var("UNAOS_VM_IMAGE").is_ok() {
        let git7 = std::env::var("UNAOS_VM_GIT7").unwrap_or_else(|_| "0000000".into());
        let img = vm_image::build(&target_dir, &esp_dir, &git7);
        println!("✅ VM image packaged at {}", img.display());
        return;
    }

    // Package-only mode: build + pack the ESP, then stop (no QEMU). Used to produce real-hardware
    // boot media — copy this directory's contents onto a FAT32 USB and boot the Mac via Option.
    if std::env::var("UNAOS_PACKAGE_ONLY").is_ok() {
        println!(
            "✅ x86_64 ESP packaged at {} (EFI/BOOT/BOOTX64.EFI + kernel.elf)",
            esp_dir.display()
        );
        return;
    }

    println!("🔹 Locating OVMF (x86_64 UEFI Firmware)...");
    // Search is additive across platforms: macOS/Homebrew first (where this may run
    // for fast iteration), then the original Linux locations (unchanged behavior).
    let ovmf_code_paths = [
        "/usr/local/share/qemu/edk2-x86_64-code.fd",   // macOS Homebrew (Intel)
        "/opt/homebrew/share/qemu/edk2-x86_64-code.fd", // macOS Homebrew (Apple Silicon)
        "/usr/share/OVMF/OVMF_CODE.fd",
        "/usr/share/edk2/ovmf/OVMF_CODE.fd",
        "/usr/share/edk2-ovmf/x64/OVMF_CODE.fd",
        "/usr/share/qemu/OVMF.fd",
    ];
    // Matching writable variable store. Split-firmware setups (macOS Homebrew, modern
    // Linux) need this as a second pflash unit; if none is found we fall back to a
    // single read-only code pflash (the original behavior).
    let ovmf_vars_paths = [
        "/usr/local/share/qemu/edk2-i386-vars.fd",
        "/opt/homebrew/share/qemu/edk2-i386-vars.fd",
        "/usr/share/OVMF/OVMF_VARS.fd",
        "/usr/share/edk2/ovmf/OVMF_VARS.fd",
        "/usr/share/edk2-ovmf/x64/OVMF_VARS.fd",
    ];

    let ovmf_code = ovmf_code_paths.iter().find(|p| std::path::Path::new(p).exists())
        .copied().expect("CRITICAL ERROR: OVMF code firmware not found.");

    // Copy the vars template to a writable per-run location (never write the template).
    let ovmf_vars = ovmf_vars_paths.iter().find(|p| std::path::Path::new(p).exists()).copied();
    let vars_writable = ovmf_vars.map(|template| {
        let dst = target_dir.join("OVMF_VARS.fd");
        std::fs::copy(template, &dst).expect("Failed to copy OVMF vars template");
        dst
    });
    println!("   code: {}  vars: {}", ovmf_code,
        vars_writable.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "<none>".into()));

    // UNA-22-HAUL: Create a phantom drive (64MB)
    let usb_image = workspace_dir.join("builder/usb.img");
    if !usb_image.exists() {
        let mut file = std::fs::File::create(&usb_image).unwrap();
        file.set_len(64 * 1024 * 1024).unwrap(); // 64MB Sparse File

        // UNA-22-MANIFEST: Inject Signature
        use std::io::Write;
        file.write_all(b"UNA-OS-DISK-001-ALPHA").unwrap();

        println!("Created usb.img (64MB) with Signature.");
    }

    // DEFAULTMEDIUM: THE DEFAULT x86 STICK, AND WHY IT IS BUILT RATHER THAN KEPT.
    //
    // `usb.img` above is the raw `UNA-OS-DISK-001-ALPHA` pattern and nothing else: no BPB, no
    // 0xAA55, so `fat::mount_source` refuses it through superfloppy, GPT and every MBR slot and the
    // boot prints `FS: no FAT filesystem (NotFat)`. NO MOUNTABLE VOLUME ON THAT MACHINE CARRIES THIS
    // KERNEL, so `:: X86BIND: root=- reason=kernel-not-found-on-any-volume … -> FAIL ::` and
    // `:: TSTE: vfsroute.refuse -> FAIL` were that medium's CORRECT answers — and the default
    // `./arroyo test` leg was therefore rc=1 on every tree, with the fold gate scoring it by PROSE
    // ("exactly those two reds are expected"). A red that is expected in prose is the shape LAWS §5
    // forbids: a leg that cannot go green can no longer say anything when something else breaks.
    //
    // The fix is a FIXTURE, not a kernel: give the default machine a volume that carries this
    // kernel, WITHOUT moving one byte of what the BOT fixture and the write probe read.
    //
    // WHAT THE TWO WITNESSES ACTUALLY TOUCH (measured, not assumed):
    //   * `crates/kernel/src/drivers/xhci/mod.rs:12936` reads LBA 0, BYTES 0..21, and prints
    //     `MISSION SUCCESS` when they spell `UNA-OS-DISK-001-ALPHA`. Bytes 0..21 of an MBR are
    //     BOOTSTRAP CODE — the partition table lives at 446 and the signature word at 510 — so the
    //     pattern and a partition table COEXIST in one sector, with no kernel-side read-offset
    //     change. They could NOT coexist with a superfloppy, whose BPB owns 0x00..0x3E; that is why
    //     the `fat-sf.img` shape can never be this leg's medium, and why the superfloppy route would
    //     have cost a kernel byte move.
    //   * `xhci/mod.rs:13091` (`usbw_keepout_ceiling`) parses that same sector and puts its scratch
    //     LBA at the top of the medium but at/above the ceiling; on a 64 MiB raw stick that is
    //     `usbw. write lba=131071 ok`. TWO things pin that number: the disk stays EXACTLY 131072
    //     sectors, and the partition must END BELOW 131071 — a volume running to the last sector
    //     raises the ceiling over every candidate and the probe skips with `on-disk container spans
    //     the medium`.
    //
    // HENCE THE GEOMETRY BELOW, every constant of which is one of those two witnesses' arithmetic:
    // a 64 MiB (131072-sector) disk; an MBR at LBA 0 carrying the pattern in its boot-code area;
    // partition 1 = FAT32 at LBA 2048..129024 (the 1 MiB alignment every formatter uses); and an
    // unallocated tail, so LBA 131071 is OUTSIDE the volume and the RMW+restore probe is genuinely
    // clear of live data rather than merely near the end of it.
    //
    // THE FILESYSTEM IS BUILT BY `scripts/make-fat-img.sh sf`, NOT BY HAND. Its `sf` layout emits a
    // bare FAT32 volume with no partition table — exactly the payload an MBR partition wants — and
    // it stages the SAME content tree the `test-fat sf` fixture is calibrated against (the ESP's
    // kernel.elf, with BOOTX64.EFI renamed to .REM so OVMF still boots the ide-hd ESP at
    // bootindex=0, plus HELLO.BIN / APPS/*.ELF / SCRATCH.BIN / GROW.BIN / S8W.BIN / BLOCK.TXT and
    // the LFN + nested-directory fixtures). ONE content tree and ONE formatter, so the default leg
    // and the sf leg cannot drift apart. Its `part` layout is deliberately NOT used: it needs
    // `sfdisk` (absent on this bench — `./arroyo fat-img` dies there today) and its partition always
    // runs to the last sector, which is precisely the geometry that kills `usbw`. The sixteen bytes
    // of partition entry are written here instead; that is the whole of what sfdisk was for.
    //
    // REBUILT EVERY RUN, and that is load-bearing: `fs::bootdisk` matches a candidate file by this
    // build's early `.text` window AND its build stamp, so an image cached from an older kernel is
    // not merely stale — it is a volume that does not carry THIS kernel, and the leg would red
    // exactly as it does today while looking fixed.
    //
    // FAILURE IS LOUD AND RED, NEVER SILENT AND GREEN: on a host without mtools/dosfstools the
    // generator says so on stdout and the slot falls back to `usb.img`, which reds the same two rows
    // it always did. A fallback that quietly produced a green run would be this arc's own defect.
    let default_stick = match build_default_medium(&workspace_dir) {
        Ok(p) => {
            println!("   DEFAULTMEDIUM: default x86 stick {} — MBR at LBA 0 (UNA-OS-DISK-001-ALPHA at offset 0; \
                      partition 1 = FAT32, type 0x0c, LBA 2048..129024), 131072 sectors total, tail from LBA \
                      129024 unallocated (the usbw scratch at LBA 131071 lies outside the volume)", p.display());
            p
        }
        Err(why) => {
            println!("   ⚠ DEFAULTMEDIUM: could NOT build the kernel-carrying default stick ({}). Falling back to \
                      the raw pattern image {} — the boot will print `FS: no FAT filesystem (NotFat)` and \
                      X86BIND / vfsroute.refuse will FAIL, which is that medium's honest answer.",
                     why, usb_image.display());
            usb_image.clone()
        }
    };

    // UNAOS_FATIMG selects a FAT filesystem image (built by scripts/make-fat-img.sh) as the
    // usb-storage backing instead of the raw UNA-OS pattern image, giving the kernel's read-only
    // FAT reader (`ls`/`cat`) a real FAT32 volume to parse. `1`/`part` -> builder/fat.img,
    // `sf` -> builder/fat-sf.img, or an explicit path. Unset (the default) is the DEFAULTMEDIUM
    // stick built above, which carries the BOT "MISSION SUCCESS" pattern in its MBR boot-code area
    // AND a kernel-carrying FAT32 volume in partition 1. block::info() registers this single device,
    // mirroring a real single-stick metal boot where the FAT32 ESP stick *is* the block device.
    let stick_image = match std::env::var("UNAOS_FATIMG").ok().as_deref() {
        None | Some("") => default_stick.clone(),
        Some("1") | Some("part") => workspace_dir.join("builder/fat.img"),
        Some("gpt") => workspace_dir.join("builder/fat-gpt.img"),
        Some("p16") => workspace_dir.join("builder/fat16.img"),
        Some("sf") => workspace_dir.join("builder/fat-sf.img"),
        Some(path) => std::path::PathBuf::from(path),
    };
    // INSTALL-CORE (UNAOS_INSTALLDEMO=1): the installer engine's target is a DEDICATED BLANK scratch
    // disk — NOT usb.img and NOT any FAT image. Create a fresh, all-zero 128 MiB image in target/ each
    // run (truncated to zero length first, so it is provably blank for the engine's blank-check
    // guard), and back the usb-storage slot with it. The boot ESP stays on the separate ide-hd, so
    // the engine writes ONLY this scratch disk. This OVERRIDES UNAOS_FATIMG (the two are exclusive:
    // the installer demo owns the block device). Not a genuinely-second drive because the block layer
    // is single-device (a second usb-storage would need xHCI multi-device support, out of this arc's
    // lane); reusing the usb-storage slot as the scratch keeps the boot ESP cleanly separate.
    let installdemo = std::env::var("UNAOS_INSTALLDEMO").is_ok();
    let stick_image = if installdemo {
        let scratch = target_dir.join("installscratch.img");
        let f = std::fs::File::create(&scratch).unwrap(); // create truncates -> all-zero sparse file
        f.set_len(128 * 1024 * 1024).unwrap(); // 128 MiB blank scratch
        drop(f);
        println!("   UNAOS_INSTALLDEMO: fresh BLANK 128 MiB scratch disk -> {}", scratch.display());
        scratch
    } else {
        stick_image
    };
    // PARTINSTALL: UNAOS_PART_DISK=<path> backs the usb-storage slot with a PRE-PARTITIONED GPT disk
    // instead of the blank scratch above — the fixture `scripts/make-gpt-fixture.py` builds, carrying
    // a foreign FAT volume, an APFS-signature volume, an empty target, an ESP-typed slot and an
    // undersized slot. That is the shape of the disk Peter will hand the installer on the rMBP (an
    // internal SSD he partitioned from Disk Utility, with a foreign volume still on it), and it is
    // the only way to exercise the refusals: a blank disk cannot refuse anything.
    //
    // WHY IT REUSES THE usb-storage SLOT rather than attaching a second USB disk: the block layer is
    // single-device over xHCI here (the UNAOS_INSTALLDEMO note above), so a genuinely-second
    // usb-storage would need multi-device support this arc does not build. The boot ESP stays on the
    // separate `ide-hd` at bootindex=0, so the fixture is still NOT the boot device and INSTALL-SELF
    // is still asked about a disk it can answer for.
    //
    // A FRESH COPY EVERY RUN, into target/: the fixture is written to by design, and a leg that
    // mutated the checked-in image would pass once and then measure a disk the previous run left
    // behind — the re-census and the neighbours-untouched sha would both be reading yesterday's
    // result. Copy, never open in place.
    //
    // BUILDER KNOB, not a kernel feature: it changes what QEMU attaches and adds not one byte to any
    // image, so the four-place knob wiring (arroyo map / builder read / k8-reach.registry row /
    // arm_features strip) does not apply — the same standing as UNAOS_AHCI_DISK below. The KERNEL
    // half of this arc rides the existing `installdemo` feature and introduces no knob of its own.
    // UNSET => not one argument changes and a default run is byte-identical.
    let stick_image = match std::env::var("UNAOS_PART_DISK") {
        Ok(src) => {
            let src = if src.is_empty() {
                workspace_dir.join("builder/part-fixture.img")
            } else {
                let p = std::path::PathBuf::from(&src);
                if p.is_relative() { workspace_dir.join(p) } else { p }
            };
            if !src.exists() {
                panic!("UNAOS_PART_DISK set but {} is missing — run \
                        `python3 scripts/make-gpt-fixture.py` from unaos/ first", src.display());
            }
            let dst = target_dir.join("partfixture.img");
            std::fs::copy(&src, &dst).unwrap();
            println!("   UNAOS_PART_DISK: fresh copy of the GPT fixture {} -> {} (partition-install target; overrides the blank scratch)",
                     src.display(), dst.display());
            dst
        }
        Err(_) => stick_image,
    };
    if stick_image != default_stick && !stick_image.exists() {
        panic!("UNAOS_FATIMG set but {} is missing — run `./arroyo fat-img` first",
            stick_image.display());
    }
    println!("   usb-storage backing: {}", stick_image.display());

    // Network backend selector. Default is user-mode (slirp): zero privileges, link
    // comes up, but the host cannot arping/ping the guest (it's NAT). UNAOS_NET=vmnet
    // switches to a vmnet-host netdev so host and guest share an L2 segment — enabling
    // `ping`/`arping 10.0.2.15` from the host. vmnet needs root, so QEMU runs under sudo.
    let net_mode = std::env::var("UNAOS_NET").unwrap_or_default();
    let use_vmnet = net_mode == "vmnet";

    println!("🔹 Launching QEMU{}...", if use_vmnet { " (vmnet-host, via sudo)" } else { "" });
    let mut cmd = if use_vmnet {
        let mut c = Command::new("sudo");
        c.arg("qemu-system-x86_64");
        c
    } else {
        Command::new("qemu-system-x86_64")
    };

    // Q35/ICH9 chipset: PCIe-based, closer to real modern hardware (e.g. a 2012 MacBook
    // Pro) than the legacy i440FX default. Note: on Q35 the qemu-xhci PCIe INTx routes to
    // an APIC GSI the 8259 PIC cannot service, so interrupt-driven xHCI uses MSI-X (local
    // APIC) rather than legacy INTx.
    // UNAOS_QEMU_MACHINE overrides the pinned machine type for a box whose QEMU predates 10.0
    // (a cloud container with QEMU 8.2 has `pc-q35-8.2`); unset, the pin stands.
    cmd.arg("-machine").arg(std::env::var("UNAOS_QEMU_MACHINE").unwrap_or_else(|_| "pc-q35-10.0".to_string()));

    // CPU model: advertise x2APIC (the default qemu64 model does not), so the kernel exercises
    // the MSR-based local-APIC path that the target hardware (2012 MacBook, Zenbook S16) uses.
    // Override with UNAOS_CPU — e.g. `UNAOS_CPU=qemu64` drops x2APIC to test the xAPIC fallback.
    let cpu = std::env::var("UNAOS_CPU").unwrap_or_else(|_| "qemu64,+x2apic".into());
    cmd.arg("-cpu").arg(cpu);

    // Firmware: read-only code pflash (unit 0) + writable vars pflash (unit 1) when a
    // vars store exists; otherwise a single read-only code pflash (legacy behavior).
    cmd.arg("-drive").arg(format!("if=pflash,unit=0,format=raw,readonly=on,file={}", ovmf_code));
    if let Some(ref vars) = vars_writable {
        cmd.arg("-drive").arg(format!("if=pflash,unit=1,format=raw,file={}", vars.display()));
    }

    // Boot the ESP first, explicitly (bootindex=0). When UNAOS_FATIMG points the usb-storage at a
    // FAT image (which also carries an EFI/ tree), OVMF would otherwise sometimes *attempt* the USB
    // drive before the ESP; that boot-time USB touch destabilizes the kernel's later BOT reads of
    // the same device (flaky FAT mounts). Pinning the ESP to bootindex=0 (and the stick to a lower
    // priority) makes OVMF go straight to the ESP, leaving the usb-storage pristine for the kernel —
    // the raw usb.img default already behaved this way (OVMF never boots a non-FAT device).
    // qemu-xhci defaults to p2=4 USB2 ports. With the FTDI attached (U2.5) there are four full/high-
    // speed USB2 devices — storage + kbd + tablet + serial — and QEMU overflows the 4th onto an
    // AUTO-INSERTED usb-hub, putting the FTDI behind a hub. Hub-downstream enumeration handles
    // HID + mass storage but not FTDI, so a hubbed FTDI is never configured. Widen the root-port
    // count so every device lands
    // on a root port — but ONLY when the FTDI is attached, so the default (no-knob) runs keep the
    // exact 4/4 controller shape and a byte-identical boot log.
    let usbserial = std::env::var("UNAOS_USBSERIAL").is_ok();
    let xhci_dev = if usbserial { "qemu-xhci,id=xhci,p2=8,p3=8" } else { "qemu-xhci,id=xhci" };
    // UNAOS_NOSTORAGE=1 omits the usb-storage device entirely, so the kernel enumerates NO block device
    // (block::info() -> None) — the QEMU analog of the metal 2012 rMBP, where the SD reader never enumerates
    // over xHCI (the storage-enumeration blocker). Used to exercise the no-storage control path: the
    // storage-INDEPENDENT capability demos (U5x/U7x/U8x — inline console-cap blobs) run + print there, while
    // every storage-GATED arc (U2/U4x/U6x/U6bx/U9x/U11x) skips. Previews exactly what the metal FTDI console
    // shows. NOTE: `is_ok()` treats an EMPTY value as SET (the known knob trap) — `UNAOS_NOSTORAGE=` is ON.
    let nostorage = std::env::var("UNAOS_NOSTORAGE").is_ok();
    // UNAOS_HUBSTORAGE=1 attaches the usb-storage BEHIND a usb-hub instead of on a root port —
    // the QEMU reproduction of the metal rMBP failure mode where the SD reader sits downstream of
    // a hub and used to be left `class=0x0`/unconfigured (hub-downstream enumeration was HID-only).
    // Exercises the hub-downstream mass-storage path end to end (interface-level MSC detect +
    // Configure-Endpoint + BOT). NOTE: `is_ok()` — an EMPTY value is ON, like the other knobs.
    let hubstorage = std::env::var("UNAOS_HUBSTORAGE").is_ok();
    cmd.arg("-drive").arg(format!("if=none,id=esp,format=raw,file=fat:rw:{}", esp_dir.display()))
       .arg("-device").arg("ide-hd,drive=esp,bootindex=0")
       .arg("-device").arg("isa-debug-exit,iobase=0xf4,iosize=0x04")
       .arg("-device").arg(xhci_dev);
    // HDA (B127) fixture: with UNAOS_HDA=1 (or UNAOS_HDATONE=1, which implies it) attach QEMU's
    // `intel-hda` controller with one `hda-duplex` codec on it and a NULL audio backend. This is
    // the one driver in this arc's neighbourhood that has a real emulator — unlike the BCM4331
    // radio or the Apple SMC, whose arcs are metal-first by construction — so the walk and the
    // stream are both gateable here, and the go-red mutation (wrong format, or stream tag 0) is a
    // measured leg rather than a reading of the source.
    //   `-audiodev none,id=snd0` is a real backend that consumes samples and produces silence, so
    // the stream engine runs, LPIB advances and BCIS latches with nothing reaching the host's
    // sound card. `audiodev=` is a REQUIRED property of the codec device on the QEMU this tree
    // builds against (`qemu-system-x86_64 -device help` lists hda-duplex on bus HDA; `-audiodev
    // help` lists `none` first), and a codec without it refuses to realise.
    //   BUILDER KNOB SHARED WITH A KERNEL FEATURE, which is the one shape the four-place wiring
    // note above does NOT cover: the same env var both pushes `hda` into `feats` and attaches the
    // device, so the fixture and the driver can never disagree about whether this run has a
    // controller in it. UNSET => not one argument is added and a default run's QEMU command line
    // is byte-identical to what it was before this arc.
    if std::env::var("UNAOS_HDA").is_ok() || std::env::var("UNAOS_HDATONE").is_ok() {
        cmd.arg("-device").arg("intel-hda,id=hda0")
           .arg("-device").arg("hda-duplex,bus=hda0.0,audiodev=snd0")
           .arg("-audiodev").arg("none,id=snd0");
        println!("   UNAOS_HDA: intel-hda controller + hda-duplex codec on a null audiodev — the HDA driver's QEMU target (walk + stream; no host sound card is opened)");
    }
    // EHCI-1 scout (UNAOS_EHCISCOUT=1): give the read-only EHCI probe a QEMU target. q35's default
    // device set has no EHCI, so attach a standalone `usb-ehci` PCI controller (class 0x0C0320) — no
    // downstream device, so the scout reports the controller's cap/op/PORTSC state with 0 connected
    // ports (the honest QEMU result). This is a QEMU-harness knob, not a kernel write path.
    // EHCI-4 M1: the driver is DEFAULT-ON, so the harness usb-ehci controller + the usb-kbd-on-ehci
    // routing ride by default; UNAOS_NOEHCIHID=1 restores the pre-fold harness (kbd on xHCI, no EHCI
    // controller unless a scout knob asks for one).
    let ehcihid = std::env::var("UNAOS_NOEHCIHID").is_err();
    if std::env::var("UNAOS_EHCISCOUT").is_ok() || std::env::var("UNAOS_EHCICONFIG").is_ok() || ehcihid {
        // IOAPIC2 (rmbp-ledger B191): under UNAOS_IOAPIC the controller sits where the 7-series PCH
        // puts EHCI #1 — 0:29.0, device 0x1d — so the ICH9 LPC's D29IR and PIRQ[n]_ROUT registers
        // describe it and `ioapic::pirq_gsi` routes it the way it routes the rMBP's 0:29.0. Anywhere
        // else it lands on QEMU's next free slot (0:3.0), which no chipset register describes, and
        // only firmware's Interrupt Line could route it. Unarmed runs are byte-for-byte unchanged.
        let ioapic_lane = std::env::var("UNAOS_IOAPIC").is_ok();
        cmd.arg("-device").arg(if ioapic_lane { "usb-ehci,id=ehci,addr=1d.0" } else { "usb-ehci,id=ehci" });
        if ioapic_lane {
            println!("   UNAOS_IOAPIC: usb-ehci placed at 0:29.0 (addr=1d.0), the PCH's EHCI #1 slot — the chipset PIRQ route's QEMU target");
        }
        if ehcihid {
            println!("   EHCI-HID (default-on): usb-ehci controller attached — EHCI HID driver target (usb-kbd rides the ehci bus; UNAOS_NOEHCIHID=1 to opt out)");
        } else if std::env::var("UNAOS_EHCICONFIG").is_ok() {
            println!("   UNAOS_EHCICONFIG: usb-ehci controller attached — EHCI configure-and-relook scout target");
        } else {
            println!("   UNAOS_EHCISCOUT: usb-ehci controller attached — read-only EHCI scout target");
        }
    }
    if nostorage {
        println!("   UNAOS_NOSTORAGE: usb-storage omitted — kernel sees no block device (metal-like no-storage path)");
    } else if hubstorage {
        println!("   UNAOS_HUBSTORAGE: usb-storage attached behind a usb-hub (hub-downstream MSC path)");
        cmd.arg("-drive").arg(format!("if=none,id=stick,format=raw,file={}", stick_image.display()))
           .arg("-device").arg("usb-hub,bus=xhci.0,port=1,id=hub0")
           .arg("-device").arg("usb-storage,bus=xhci.0,port=1.1,drive=stick,bootindex=1");
    } else {
        cmd.arg("-drive").arg(format!("if=none,id=stick,format=raw,file={}", stick_image.display()))
           .arg("-device").arg("usb-storage,bus=xhci.0,drive=stick,bootindex=1");
    }
    // STORSLOT: UNAOS_USB2=<sf|1|part|gpt|p16|path> attaches a SECOND usb-storage device on the
    // SAME xHCI controller, so the fixture can carry a boot stick AND a target/friend disk at once.
    //
    // WHY THIS COULD NOT EXIST BEFORE. The two blocks above (UNAOS_INSTALLDEMO, UNAOS_PART_DISK)
    // both say so in prose and both act on it: they REUSE the one usb-storage slot rather than add
    // a second, because the xHCI driver held ONE `storage_slot` and a second mass-storage device
    // was overwritten on the root path and refused outright on the hub path. The driver tracks an
    // array of storage records now (`drivers/xhci/mod.rs`, `StorageRecord`), and the block registry
    // below it has been an array since USBREG — so a second stick enumerates, is brought up on its
    // own main-loop pass, publishes at registry index 1 and is reachable as `BlockSource::UsbN(1)`.
    //
    // THE GRAMMAR MIRRORS THE FIRST STICK'S. `sf` is `builder/fat-sf.img`, `1`/`part` is
    // `builder/fat.img`, `gpt` is `builder/fat-gpt.img`, `p16` is `builder/fat16.img`, anything else
    // is a path — the same table `UNAOS_FATIMG` reads, so one fixture line reads the same both ways.
    //
    // A FRESH COPY EVERY RUN, into target/ (the UNAOS_PART_DISK discipline, for two reasons here).
    // First, the two sticks are routinely the SAME layout, and QEMU cannot open one raw file twice
    // for writing — the run would die on the image lock instead of attaching a second disk. Second,
    // a guest that writes to the second stick would mutate a checked-in fixture, so the next run
    // would measure yesterday's result.
    //
    // NO `bootindex`: the ESP keeps bootindex=0 and the first stick bootindex=1, so OVMF's boot
    // order is exactly what it is today and a second disk can never win the boot. Under
    // UNAOS_NOSTORAGE the knob is refused out loud rather than quietly turned into a one-stick run —
    // NOSTORAGE means "the kernel sees no block device", and a second stick would answer a different
    // question than the one that control leg asks.
    //
    // BUILDER KNOB, not a kernel feature: it changes what QEMU attaches and adds not one byte to any
    // image, so the four-place knob wiring (arroyo map / builder read / k8-reach.registry row /
    // arm_features strip) does not apply — the same standing as UNAOS_PART_DISK and UNAOS_AHCI_DISK
    // above. UNSET => not one argument is added and a default run's QEMU command line is
    // byte-identical to what it was before this arc.
    if let Ok(usb2) = std::env::var("UNAOS_USB2") {
        if nostorage {
            println!("   UNAOS_USB2: REFUSED — UNAOS_NOSTORAGE is set, so no usb-storage is attached at all (the no-block-device control leg); unset one of the two");
        } else {
            let src = match usb2.as_str() {
                "" | "1" | "part" => workspace_dir.join("builder/fat.img"),
                "gpt" => workspace_dir.join("builder/fat-gpt.img"),
                "p16" => workspace_dir.join("builder/fat16.img"),
                "sf" => workspace_dir.join("builder/fat-sf.img"),
                path => {
                    let p = std::path::PathBuf::from(path);
                    if p.is_relative() { workspace_dir.join(p) } else { p }
                }
            };
            if !src.exists() {
                panic!("UNAOS_USB2 set but {} is missing — run `./arroyo fat-img` from unaos/ first",
                    src.display());
            }
            let dst = target_dir.join("usb2.img");
            std::fs::copy(&src, &dst).unwrap();
            cmd.arg("-drive").arg(format!("if=none,id=stick2,format=raw,file={}", dst.display()))
               .arg("-device").arg("usb-storage,bus=xhci.0,drive=stick2");
            println!("   UNAOS_USB2: SECOND usb-storage on the xHCI — fresh copy of {} -> {} (no bootindex; the ESP keeps bootindex=0 and the first stick bootindex=1)",
                     src.display(), dst.display());
        }
    }
    // AHCI (B89) fixture: UNAOS_AHCI_DISK=<path> attaches a SECOND SATA disk to q35's built-in ICH9
    // AHCI controller, which QEMU exposes as the `ide.N` buses. The ESP already sits on `ide.0`
    // (`ide-hd,drive=esp,bootindex=0` above), so this lands on `ide.1` — an explicit bus, not the
    // first-free default, so the ESP's port assignment cannot move under it and the boot is
    // unaffected. No `bootindex`: OVMF must keep booting the ESP exactly as it does today, and a
    // fixture disk that could win the boot order would change what every other leg measures.
    // UNSET => not one argument is added and a default run's QEMU command line is byte-identical to
    // what it was before this arc, which is the property that lets this knob exist at all.
    // The fixture the DONE gate uses is `builder/fat-gpt.img` (`scripts/make-fat-img.sh gpt`): a
    // GPT-partitioned FAT32 disk, so the driver's sector-0 witness reads `kind=GPT` off a real
    // protective MBR rather than off a synthetic one.
    //
    // AHCIWRITE: WHEN THE WRITE KNOB IS ARMED THE ATTACHED FILE IS A FRESH COPY, NEVER THE ORIGINAL.
    // PARTINSTALL already learned this on the USB side (see UNAOS_PART_DISK above): a leg that wrote
    // into the checked-in fixture would pass once and then measure the disk the previous run left
    // behind — the re-census, the neighbours sha and the other-disk fingerprint would all be reading
    // yesterday's result, which is the single most convincing kind of wrong. It is also a safety
    // property in its own right and that is the larger half: with the write path armed, QEMU is
    // never pointed at a file the operator handed it directly, so a mistyped path cannot make the
    // guest write over something that matters. READ-ONLY runs (knob unset) attach the file as-is,
    // exactly as they did before this arc, so no existing leg's command line changes.
    if let Ok(ahci_disk) = std::env::var("UNAOS_AHCI_DISK") {
        let src = {
            let p = std::path::PathBuf::from(&ahci_disk);
            if p.is_relative() { workspace_dir.join(p) } else { p }
        };
        if !src.exists() {
            panic!("UNAOS_AHCI_DISK set but {} is missing — run \
                    `python3 scripts/make-gpt-fixture.py` from unaos/ first", src.display());
        }
        let attach = if std::env::var("UNAOS_AHCI_WRITE").is_ok() {
            let dst = target_dir.join("ahcifixture.img");
            std::fs::copy(&src, &dst).unwrap();
            println!("   UNAOS_AHCI_WRITE: fresh copy of the SATA fixture {} -> {} (the guest can write this disk; the source is never opened)",
                     src.display(), dst.display());
            dst
        } else {
            src
        };
        cmd.arg("-drive").arg(format!("if=none,id=sata0,format=raw,file={}", attach.display()))
           .arg("-device").arg("ide-hd,drive=sata0,bus=ide.1");
        println!("   UNAOS_AHCI_DISK: second SATA disk on ide.1 ({}) — AHCI driver target (ESP keeps ide.0 and bootindex=0)", attach.display());
    }
    // EHCI-3 harness: by default (EHCI-4 M1 driver on) the keyboard rides the EHCI bus (QEMU's usb-kbd is
    // HS-capable, so it trains directly on the EHCI root port — Topology B). It REPLACES the
    // xHCI keyboard in this mode so QMP `send-key` routes deterministically to the EHCI device
    // (two keyboards would leave the routing to QEMU's whim). QEMU cannot model the RMH hub
    // tier: its only hub is full-speed and wedges the machine at firmware if placed on the EHCI
    // bus — Topology A (hub walk + splits) is metal-first by construction.
    // XHCIKBD (B45): UNAOS_XHCIKBD=1 puts the ONE usb-kbd on the xHCI bus instead, with the EHCI driver
    // still compiled and running (its bus simply carries no keyboard) — a NEW LEG for the shared xHCI
    // keyboard decoder, not a swap of the default, and not NOEHCIHID. One keyboard, never two: with two
    // QEMU routes `input-send-event`/`send-key` to whichever handler it likes, and the fixture's count
    // would then be a fact about QEMU's routing. Kept in sync with arroyo.
    let xhcikbd = std::env::var("UNAOS_XHCIKBD").is_ok();
    if ehcihid && !xhcikbd {
        cmd.arg("-device").arg("usb-kbd,bus=ehci.0");
    } else {
        cmd.arg("-device").arg("usb-kbd,bus=xhci.0");
        if xhcikbd {
            println!("   UNAOS_XHCIKBD: usb-kbd on the xHCI bus (EHCI driver still on, no keyboard on it) — XHCIKBD burst-fixture target");
        }
    }
    // EHCI-4 M2 gate (UNAOS_EHCITABLET=1): move the usb-tablet onto the EHCI bus so the driver's
    // report-protocol POINTER path is exercised end-to-end — QEMU's usb-tablet is a non-boot
    // (proto 0) absolute pointer, exactly the trackpad shape, so the driver reads + parses its HID
    // report descriptor (GET_DESCRIPTOR(Report)), arms a report-protocol interrupt-IN QH, and
    // decodes X/Y/buttons to pal::Event::MouseAbsolute. Default: tablet stays on xHCI (the xHCI HID
    // tests keep their pointer). Only meaningful with the driver active (ignored under NOEHCIHID).
    let ehci_tablet = ehcihid && std::env::var("UNAOS_EHCITABLET").is_ok();
    // XHCIHUB (rmbp 2026-09-15, LEDGER S1/S2): UNAOS_XHCIHUB=1 MOVES the xHCI pointer behind a
    // `usb-hub` on a root port instead of attaching it to the root port directly — the QEMU
    // reproduction of the two hub defects the Orin bench carries: the hub's interrupt-IN
    // status-change endpoint failing Configure-Endpoint (S1), and a hub-attached pointer whose
    // `MOUSE-1` witness prints `vid:pid=0000:0000` (S2). It MOVES rather than ADDS because two
    // pointers would leave which one QMP `input-send-event` reaches to QEMU's routing — the same
    // "one keyboard, never two" reasoning as the XHCIKBD block above, and the fixture's whole point
    // is that the pointer under test is the HUB-DOWNSTREAM one. The keyboard is NOT moved: it keeps
    // its default EHCI bus so `[hidkeys]` measures exactly what it measured before.
    // Port 4 is chosen explicitly: qemu-xhci's default `p2=4` puts USB2 ports at 1..4 (QEMU's only
    // hub model is FULL-SPEED, so it must land on a USB2 port), the auto-assigned devices above take
    // the lowest free ports, and UNAOS_HUBSTORAGE's hub already claims port 1 — so port 4 cannot
    // collide with either. NOTE: `is_ok()` — an EMPTY value is ON, like every other knob here.
    // UNSET => not one argument changes and the QEMU command line is byte-identical.
    let xhcihub = !ehci_tablet && std::env::var("UNAOS_XHCIHUB").is_ok();
    if ehci_tablet {
        cmd.arg("-device").arg("usb-tablet,bus=ehci.0");
        println!("   UNAOS_EHCITABLET: usb-tablet on the EHCI bus — EHCI-4 M2 report-pointer path target");
    } else if xhcihub {
        cmd.arg("-device").arg("usb-hub,bus=xhci.0,port=4,id=hubh")
           .arg("-device").arg("usb-tablet,bus=xhci.0,port=4.1");
        println!("   UNAOS_XHCIHUB: usb-hub on xHCI root port 4 with the usb-tablet behind it (port 4.1) — XHCIHUB hub status-change + hub-downstream pointer fixture");
    } else {
        cmd.arg("-device").arg("usb-tablet,bus=xhci.0");
    }
    // BATMON-1 (UNAOS_SMC=1): attach QEMU's ISA AppleSMC so the SMC driver has a protocol target.
    // The emulated device answers the polled key/value protocol on iobase 0x300 with a tiny key set
    // (REV/OSK0/OSK1 + a few status keys) — enough to gate the read-key machinery via a known-key
    // read. It carries NO battery keys and implements neither #KEY nor GET_KEY_BY_INDEX, so those
    // stay metal-first (the driver reports them cleanly absent on QEMU). The `osk` here is a
    // deliberately fake placeholder (not Apple's key) — irrelevant to reading REV, and this harness
    // never boots macOS. Only attached under the knob, so default media/QEMU runs are unchanged.
    if std::env::var("UNAOS_SMC").is_ok() {
        cmd.arg("-device").arg(
            "isa-applesmc,osk=UNAOSisNOTaMACplaceholderOSKxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
        );
        println!("   UNAOS_SMC: isa-applesmc attached (iobase 0x300) — SMC protocol/read-key target");
    }
    cmd.arg("-m").arg("1G");

    // U2.5 (UNAOS_USBSERIAL): attach an FTDI FT232 usb-serial device on the xHCI bus. QEMU's
    // `-device usb-serial` emulates an FT232 (VID 0x0403 PID 0x6001, bulk IN 0x81 / OUT 0x02); its
    // chardev is a file at target/ftdi.log, so the kernel's FTDI console driver enumerates it and
    // replays the boot log out bulk-OUT — the metal cable (arriving ~2026-07-08) behaves the same.
    // NOTE: `is_ok()` treats an EMPTY value as SET (the known knob trap) — `UNAOS_USBSERIAL=` is ON.
    //
    // FTDIRX (rmbp A9): a `file` chardev CANNOT BE WRITTEN INTO. That is fine for a TX-only console
    // and fatal for an RX gate — there is no way to type at the emulated cable. `UNAOS_FTDIRX_INJECT`
    // = a UNIX socket path swaps the file for a LISTENING socket chardev, which is bidirectional:
    // `scripts/ftdi_inject.py <path>` connects to it once the console is up and writes the bytes the
    // kernel must receive, and everything QEMU would have written to the file comes back over the
    // same socket instead. DEFAULT UNSET keeps today's file chardev BYTE-FOR-BYTE, so every existing
    // U2.5 run and its `target/ftdi.log` capture are untouched. `server=on,wait=off` so QEMU creates
    // the socket and boots WITHOUT waiting for a peer — the injector attaches seconds later, mid-boot,
    // exactly as a bench operator plugs into a running machine.
    if usbserial {
        match std::env::var("UNAOS_FTDIRX_INJECT") {
            Ok(sock) if !sock.is_empty() => {
                let _ = std::fs::remove_file(&sock); // a stale socket file makes QEMU refuse to bind
                cmd.arg("-chardev").arg(format!("socket,id=ftdi0,path={sock},server=on,wait=off"))
                   .arg("-device").arg("usb-serial,bus=xhci.0,chardev=ftdi0");
                println!("   U2.5 + FTDIRX: FTDI usb-serial attached; console is a UNIX socket -> {sock}");
            }
            _ => {
                let ftdi_log = target_dir.join("ftdi.log");
                let _ = std::fs::remove_file(&ftdi_log); // start each run with a fresh capture file
                cmd.arg("-chardev").arg(format!("file,id=ftdi0,path={}", ftdi_log.display()))
                   .arg("-device").arg("usb-serial,bus=xhci.0,chardev=ftdi0");
                println!("   U2.5: FTDI usb-serial attached; console capture -> {}", ftdi_log.display());
            }
        }
    }

    // SMP: bring up multiple CPUs so the kernel's AP-startup path has application
    // processors to discover (ACPI MADT) and boot (INIT-SIPI-SIPI). Override the core
    // count with UNAOS_SMP (e.g. `UNAOS_SMP=1` to force uniprocessor). The BSP still
    // drives xHCI/console/storage; APs idle until the scheduler work lands.
    // WITCORE: 6, not 4. SCHED-X86 spends two APs (render takes the pool's head, the device service
    // its tail), so with `-smp 4` = 3 APs the non-render pool is exactly ONE core and every fixture
    // that needs three distinct non-render cores stops running: u7x, u6gx, sock4, and irqstorage's
    // bx-blockreq. u6gx is the only automated exercise of the STOR-1 S5 mitigation (owner A
    // busy-spinning on the storage-service core) — i.e. precisely the interaction the placement rule
    // exists to protect. 6 restores index-2 consumers to a 3-core pool. Metal is this track's
    // verdict; this line is here so the change does not silently delete fixture coverage.
    //
    // 6 meets the requirement with ZERO SLACK: 5 APs - render - service = exactly 3. One AP failing
    // INIT-SIPI-SIPI (`smp.rs` logs `did not come online (timeout)`) drops the pool to 2 and those
    // fixtures skip again. `:: SCHED-X86 PLACE-CHECK: ... verdict=PARTIAL ::` is the line that says
    // so — not a FAIL, because the placement rule still holds; it is a COVERAGE loss. Raise this
    // number rather than reading PARTIAL as noise.
    let smp = std::env::var("UNAOS_SMP").unwrap_or_else(|_| "6".into());
    cmd.arg("-smp").arg(smp);

    // Network: Intel e1000 (82540EM). slirp (default) brings the link up but the host
    // can't reach the guest (NAT). vmnet-host shares an L2 segment on 10.0.2.0/24
    // (host = 10.0.2.1) so `ping`/`arping` of the guest's static 10.0.2.15 works.
    // For slirp wire debugging add UNAOS_QEMU_EXTRA="-object filter-dump,id=d0,netdev=n0,file=target/net.pcap".
    if use_vmnet {
        cmd.arg("-netdev")
            .arg("vmnet-host,id=n0,start-address=10.0.2.1,end-address=10.0.2.254,subnet-mask=255.255.255.0");
    } else if net_mode == "socket" {
        // Rootless L2 link to a host injector (scripts/net-inject.py) for automated
        // ARP/ICMP responder testing without privileges. QEMU listens; injector connects.
        cmd.arg("-netdev").arg("socket,id=n0,listen=127.0.0.1:5555");
    } else {
        // dhcpstart shifts slirp's DHCP pool so a successful lease (10.0.2.20) is visibly
        // distinct from the guest's static fallback (10.0.2.15) — makes DHCP easy to confirm.
        cmd.arg("-netdev").arg("user,id=n0,dhcpstart=10.0.2.20");
    }
    cmd.arg("-device").arg("e1000e,netdev=n0,mac=52:54:00:12:34:56");

    // SDHC-1 (DEFAULT-ON, opt out with UNAOS_NOSDHCI=1): attach QEMU's generic PCI SD host
    // controller (`sdhci-pci`, which reports the SAME class triple as the rMBP's reader — class
    // 0x08 / subclass 0x05) with an SD card plugged into it, so the read-only SDHC-1 discovery
    // probe has QEMU coverage: `[PCI-STOR]` sees a storage-class function and `[sdhc]` reads a
    // real Host Controller Version + Capabilities out of BAR0. Attached LAST so no existing
    // device's PCI slot assignment moves. The card image is a blank 16 MiB (power-of-two, which
    // QEMU's sd-card requires) file in target/ — milestone 1 transfers no data, it only needs the
    // slot to read as occupied so the present-state witness is not trivially empty.
    // Kept in sync with unaos/arroyo (UNAOS_NOSDHCI).
    if std::env::var("UNAOS_NOSDHCI").is_err() {
        let sd_image = target_dir.join("sdcard.img");
        // SDHC-4c: the card image is no longer blank by default — it carries a FAT16 superfloppy
        // with the host-staged reservation `UNALOG.BIN` on it, so the reserve/arm/write/read-back
        // ladder actually EXECUTES under QEMU instead of being compiled and skipped. See
        // `stage_sdhc4c_volume`. `UNAOS_SDCARD_BLANK=1` restores the old blank 16 MiB file, which
        // is the fixture for the honest-refusal path (`no FAT volume ... permit=UNARMED`).
        if std::env::var("UNAOS_SDCARD_BLANK").is_ok() {
            if !sd_image.exists() {
                let f = std::fs::File::create(&sd_image).expect("failed to create target/sdcard.img");
                f.set_len(16 * 1024 * 1024).expect("failed to size target/sdcard.img");
            }
            println!("   SDHC-4c: UNAOS_SDCARD_BLANK — card image left BLANK (exercises the `no FAT volume` refusal)");
        } else {
            stage_sdhc4c_volume(&sd_image);
        }
        cmd.arg("-device").arg("sdhci-pci,id=sdhci0")
           .arg("-drive").arg(format!("if=none,id=sdcard0,format=raw,file={}", sd_image.display()))
           // QEMU names sdhci-pci's child bus plainly `sd-bus` (hw/sd/sdhci.c), not `<id>.sd-bus`.
           .arg("-device").arg("sd-card,bus=sd-bus,drive=sdcard0");
        println!("   SDHC-1 (default-on): sdhci-pci + sd-card attached ({}) — read-only SDHCI discovery target (UNAOS_NOSDHCI=1 to opt out)",
            sd_image.display());
    }

    // DIAGNOSTIC: append arbitrary QEMU args from UNAOS_QEMU_EXTRA (whitespace-split), e.g.
    // `-d guest_errors -trace usb_xhci_* -trace usb_msd_*`, so we can capture QEMU's own
    // tracing of the xHCI/SCSI path. In test mode QEMU's stderr is redirected to a file.
    let qemu_extra = std::env::var("UNAOS_QEMU_EXTRA").unwrap_or_default();
    for a in qemu_extra.split_whitespace() {
        cmd.arg(a);
    }

    // vmnet runs under sudo; signal-based self-kill does not propagate through sudo, so
    // run interactively (GUI) with serial -> file and a wire pcap, and let the user quit
    // QEMU (Ctrl-C in this terminal / close the window) when done testing.
    if use_vmnet {
        let log_path = std::env::var("UNAOS_SERIAL_LOG")
            .unwrap_or_else(|_| target_dir.join("serial.log").display().to_string());
        let pcap = target_dir.join("net.pcap");
        // Headless (GUI-under-sudo is unreliable on macOS); serial + wire pcap to files.
        // Stop with Ctrl-C in this terminal (SIGINT reaches QEMU via the process group).
        cmd.arg("-display").arg("none")
            .arg("-serial").arg(format!("file:{log_path}"))
            .arg("-object")
            .arg(format!("filter-dump,id=d0,netdev=n0,file={}", pcap.display()));
        println!("   [vmnet] headless; serial -> {log_path}");
        println!("   [vmnet] wire pcap -> {}", pcap.display());
        println!("   [vmnet] guest IP 10.0.2.15. In another terminal, find the host vmnet iface IP:");
        println!("   [vmnet]   ifconfig | grep -B3 10.0.2     (expect a bridge/vmnet iface at 10.0.2.1)");
        println!("   [vmnet] then:  ping -c3 10.0.2.15        (ping does ARP first, so it tests ARP + ICMP)");
        println!("   [vmnet] watch replies in {log_path}  and  tcpdump -r {} -nne", pcap.display());
        println!("   [vmnet] Ctrl-C here to stop QEMU when done.");
        let mut child = cmd.spawn().unwrap();
        child.wait().unwrap();
        return;
    }

    // Test mode: set UNAOS_SERIAL_LOG to run headless, redirect serial to that file, and
    // self-terminate after UNAOS_TEST_SECS (default 20s). Keeps automated boot-log capture
    // portable (no `timeout` binary needed). Normal runs keep the GUI + serial on stdio.
    if let Ok(log_path) = std::env::var("UNAOS_SERIAL_LOG") {
        let secs: u64 = std::env::var("UNAOS_TEST_SECS").ok()
            .and_then(|s| s.parse().ok()).unwrap_or(20);
        println!("   [test mode] headless, serial -> {log_path}, auto-kill after {secs}s");
        cmd.arg("-display").arg("none")
           .arg("-serial").arg(format!("file:{log_path}"));
        // Capture QEMU's own stderr (where -d / -trace output goes) next to the serial log.
        if let Ok(dbg_path) = std::env::var("UNAOS_QEMU_DEBUG_LOG") {
            if let Ok(f) = std::fs::File::create(&dbg_path) {
                cmd.stderr(Stdio::from(f));
            }
        }
        let mut child = cmd.spawn().unwrap();
        // FASTTEST: this used to be a bare `thread::sleep(secs)` — the blind wall LAWS §5 removed
        // from every OTHER QEMU verb ("QEMU verbs exit at COMPLETION + GRACE, not at a wall") and
        // the one this verb kept, because `arroyo` hands the pid to this builder and never holds
        // it. `qemu_test_wall` is that rule arriving here: it ends the run when the capture shows
        // the run FINISHED, and sleeps out the rest of `secs` in every other case.
        qemu_test_wall(&workspace_dir, &log_path, secs);
        let _ = child.kill();
        let _ = child.wait();
    } else {
        cmd.arg("-serial").arg("stdio");
        let mut child = cmd.spawn().unwrap();
        child.wait().unwrap();
    }
}

// ===================== FASTTEST — the test-mode wall ends at COMPLETION + GRACE ==============
//
// WHY THIS LIVES IN THE BUILDER AND NOT IN `arroyo`. Every other QEMU verb waits through
// `arroyo`'s `qemu_wait_or_complete`, which can shorten a run because it OWNS the QEMU process.
// `./arroyo test` does not: it runs `cargo run` here, this program spawns QEMU, and the pid never
// leaves this function. So the verb sat out a blind `thread::sleep(secs)` — LEDGER SR7's owed
// half — while TESTTRUNC gave the same capture a completion SOURCE (`scripts/specs/x86-test.spec`)
// and used it only for a VERDICT. A source and a shortening are two different items; this is the
// second one, and the wait is plumbed through the process that holds the pid.
//
// THE PREDICATE IS NOT REIMPLEMENTED HERE, and that is the whole point of shelling out. The
// marker is read by `scripts/qemu_await.py` through `mbench`'s own `Matcher.complete()` over the
// spec `arroyo` names — the same program, the same predicate and the same `AWAIT` line the other
// verbs stop on. A matcher written a second time in Rust could disagree with the replay about
// what "the run finished" means, and a gate that disagrees with its own verdict authority is
// worse than a slow gate. (The alternative shape — `arroyo` spawning QEMU itself so
// `qemu_wait_or_complete` could hold the pid — moves the entire QEMU command line, the ESP
// staging and the knob plumbing out of this file; it is a rewrite of the run path, not a wait,
// and it would leave two spellings of the same command line to drift apart.)
//
// EVERY NON-`complete` OUTCOME PAYS THE FULL WALL, counted from BEFORE the waiter started, which
// is LAWS §5 in as many words ("a verb with no declared completion source pays the full wall, and
// every non-completing outcome pays it too"). `nosignal` returns in ~0 s and a naive fast-path
// would turn a 120 s gate into a 0 s one — the exact bug helper-unit H5 caught in the shell twin.
// `UNAOS_QEMU_FULL=1` keeps the whole wall with no waiter at all.
//
// THE `AWAIT` LINE IS WRITTEN BESIDE THE LOG at `<log>.await`, because `arroyo` derives the
// `<log>.run` sidecar's `mode=` / `completion_at=` from it and cannot see this child's stdout
// through `cargo run`'s. It is REMOVED by the first statement below — before this wall can write
// anything and long before `arroyo` reads it — so a stale line from a previous run can never be
// read as this run's: an absent file means "the fast path did not report", which `arroyo` renders
// as `mode=full`. The stamp is never a verdict; `arroyo` re-reads the capture itself for that.
fn qemu_test_wall(workspace_dir: &std::path::Path, log_path: &str, secs: u64) {
    let t0 = std::time::Instant::now();
    let stamp = std::path::PathBuf::from(format!("{log_path}.await"));
    let _ = std::fs::remove_file(&stamp);

    // Pay whatever is left of `secs` since `t0`. Saturating: a waiter that came back AFTER the
    // cap (a loaded box, a slow interpreter) owes nothing, it must not wrap into a second wall.
    let pay_the_rest = |why: &str| {
        let left = std::time::Duration::from_secs(secs).saturating_sub(t0.elapsed());
        println!("   [test mode] {why} — paying the rest of the {secs}s wall ({:.1}s).", left.as_secs_f64());
        std::thread::sleep(left);
    };

    if std::env::var("UNAOS_QEMU_FULL").ok().as_deref() == Some("1") {
        pay_the_rest("UNAOS_QEMU_FULL=1, the DONE-gate form");
        return;
    }
    // The spec path reaches us on the same channel as the wall seconds: `arroyo` exports it beside
    // UNAOS_TEST_SECS. Unset (a direct `cargo run`, or a verb that declares no source) is not an
    // error — it is the full wall, stated.
    let spec = std::env::var("UNAOS_TEST_SPEC").unwrap_or_default();
    if spec.is_empty() || !std::path::Path::new(&spec).is_file() {
        pay_the_rest("no completion spec declared for this run");
        return;
    }
    // Default 20 s, the same number as `arroyo`'s QEMU_GRACE_DEFAULT — which is the value `arroyo`
    // actually passes; this literal only covers a direct `cargo run`.
    let grace: f64 = std::env::var("UNAOS_QEMU_GRACE").ok()
        .and_then(|s| s.parse().ok()).unwrap_or(20.0);
    let awaiter = workspace_dir.join("scripts/qemu_await.py");

    println!("   [test mode] awaiting completion: {} (cap {secs}s, grace {grace:.0}s)", spec);
    let out = Command::new("python3")
        .arg(&awaiter)
        .arg("--log").arg(log_path)
        .arg("--spec").arg(&spec)
        .arg("--cap").arg(secs.to_string())
        .arg("--grace").arg(format!("{grace}"))
        .arg("--label").arg("test")
        .current_dir(workspace_dir)
        // stderr INHERITED: the waiter's human line ("run complete at +Ns — holding grace") is
        // the reader's only live sign that the run is ending early. Only stdout is captured.
        .stderr(Stdio::inherit())
        .output();

    let line = match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines().find(|l| l.starts_with("AWAIT ")).map(|l| l.to_string()),
        Err(e) => {
            eprintln!("   [test mode] ✖ cannot run {}: {e}", awaiter.display());
            None
        }
    };
    let line = match line {
        Some(l) => l,
        None => {
            // THE OUTCOME THAT MUST NOT BE SILENT: a broken waiter that fell through to the kill
            // would turn this gate into a 0 s one and judge whatever fragment of a boot was on
            // disk. Degrade to TODAY's behaviour — the full wall — and say why.
            eprintln!("   [test mode] ✖ the completion waiter produced no AWAIT line — the fast path is BROKEN, not merely unlucky; fix scripts/qemu_await.py.");
            pay_the_rest("the completion waiter gave no verdict");
            return;
        }
    };
    println!("   [test mode] {line}");
    let _ = std::fs::write(&stamp, format!("{line}\n"));
    let status = line.split_whitespace()
        .filter_map(|f| f.split_once('='))
        .find(|(k, _)| *k == "status")
        .map(|(_, v)| v.to_string())
        .unwrap_or_default();
    match status.as_str() {
        // The fast exit was EARNED: the capture contains the end of the run and the grace window
        // has already been held with every FORBID live. QEMU dies on return.
        "complete" => println!("   [test mode] run complete + {grace:.0}s grace at {:.1}s of a {secs}s cap — ending the run.", t0.elapsed().as_secs_f64()),
        // `cap` means the waiter already sat out the whole wall; the sleep below is ~0 and is left
        // in rather than special-cased, so there is exactly one place the wall is paid.
        "cap" => pay_the_rest("no completion inside the cap"),
        "nosignal" => pay_the_rest("the named spec declares no COMPLETE marker"),
        other => pay_the_rest(&format!("unexpected waiter status '{other}'")),
    }
}

// ===================== SDHC-4c — the host-staged reservation, for QEMU =====================
//
// SDHC-4c's kernel side is ADOPT-ONLY: it locates a fixed-size, contiguous file that the HOST put
// on the card's FAT volume, publishes that file's LBA extent as the only writable set on the
// medium, and refuses everything else. That design has an obvious consequence for verification —
// with a blank card image the arc's entire ladder (adopt, arm, self-test, write, read back) is
// compiled and never runs, and "it skipped honestly" is not evidence that the interesting path
// works. This function is the host half, so the ladder EXECUTES under `./arroyo test-fat`.
//
// The volume is written here in plain Rust rather than shelled out to `mkfs.vfat` + `mtools` for
// two reasons. It removes a build dependency the sandbox does not always have. And, more usefully,
// it fixes the geometry EXACTLY, so this builder can print the extent it just staged and the
// kernel's `:: SDHC4C: reserve ... lba=[A..B) ::` witness can be compared against a number that was
// computed independently, on the other side of the boot. A mismatch convicts the kernel's BPB
// arithmetic or its chain walk; agreement is a real cross-check, not a tautology.
//
// GEOMETRY (16 MiB = 32768 sectors, FAT16 superfloppy at LBA 0, no partition table):
//   bytes/sec 512, sec/clus 4 (2 KiB clusters), reserved 1, 2 FATs x 32 sectors, root dir 32 sectors
//   first_data_sector = 1 + 2*32 + 32 = 97          => cluster 2 starts at LBA 97
//   data sectors      = 32768 - 97 = 32671          => 8167 clusters (FAT16: 4085 <= n < 65525 OK)
//   FAT capacity      = 32 * 512 / 2 = 8192 entries >= 8167 + 2                    OK
//   UNALOG.BIN        = 65536 bytes = 32 clusters, laid down CONTIGUOUSLY at cluster 2
//   => the reserved extent is LBA [97 .. 225), 128 sectors. That is the number to predict.
const SD4C_TOTAL_SECTORS: u32 = 32768;
const SD4C_SEC_PER_CLUS: u32 = 4;
const SD4C_RESERVED: u32 = 1;
const SD4C_NUM_FATS: u32 = 2;
const SD4C_FAT_SECTORS: u32 = 32;
const SD4C_ROOT_ENTRIES: u32 = 512; // 512 * 32 B = 16384 B = 32 sectors
const SD4C_FIRST_CLUSTER: u32 = 2;
const SD4C_RESERVE_BYTES: u32 = 64 * 1024;
/// Bytes 3..11 of the boot sector (`BS_OEMName`). Doubles as this fixture's signature: an image
/// already carrying it is left ALONE, so whatever the previous run's kernel wrote into the reserved
/// file survives into the next boot. That is what makes "the record persisted across a reboot"
/// observable under QEMU at all, and persistence is the entire point of the arc.
const SD4C_OEM: &[u8; 8] = b"UNAOS4C ";

fn stage_sdhc4c_volume(path: &std::path::Path) {
    let root_dir_sectors = SD4C_ROOT_ENTRIES * 32 / 512;
    let first_data_sector = SD4C_RESERVED + SD4C_NUM_FATS * SD4C_FAT_SECTORS + root_dir_sectors;
    let clus_bytes = SD4C_SEC_PER_CLUS * 512;
    let need_clusters = SD4C_RESERVE_BYTES.div_ceil(clus_bytes);
    let extent_start = first_data_sector; // cluster 2 == the first data sector
    let extent_end = extent_start + need_clusters * SD4C_SEC_PER_CLUS;

    // Already staged by a previous run? Leave the DATA alone — see `SD4C_OEM`.
    if let Ok(existing) = std::fs::read(path) {
        if existing.len() == (SD4C_TOTAL_SECTORS as usize) * 512 && existing[3..11] == SD4C_OEM[..] {
            println!(
                "   SDHC-4c: card image already staged ({}) — reserved {} preserved, extent LBA [{}..{}) ({} sectors); a previous boot's record survives",
                path.display(), "UNALOG.BIN", extent_start, extent_end, extent_end - extent_start
            );
            return;
        }
    }

    let mut img = vec![0u8; (SD4C_TOTAL_SECTORS as usize) * 512];

    // ---- boot sector (BPB) ----
    let bs = &mut img[0..512];
    bs[0] = 0xEB; bs[1] = 0x3C; bs[2] = 0x90; // BS_JmpBoot — the VBR discriminator parse_bpb demands
    bs[3..11].copy_from_slice(SD4C_OEM);
    bs[11..13].copy_from_slice(&512u16.to_le_bytes());          // BPB_BytsPerSec
    bs[13] = SD4C_SEC_PER_CLUS as u8;                            // BPB_SecPerClus
    bs[14..16].copy_from_slice(&(SD4C_RESERVED as u16).to_le_bytes()); // BPB_RsvdSecCnt
    bs[16] = SD4C_NUM_FATS as u8;                                // BPB_NumFATs
    bs[17..19].copy_from_slice(&(SD4C_ROOT_ENTRIES as u16).to_le_bytes()); // BPB_RootEntCnt
    bs[19..21].copy_from_slice(&(SD4C_TOTAL_SECTORS as u16).to_le_bytes()); // BPB_TotSec16 (32768 fits)
    bs[21] = 0xF8;                                               // BPB_Media (fixed disk)
    bs[22..24].copy_from_slice(&(SD4C_FAT_SECTORS as u16).to_le_bytes()); // BPB_FATSz16
    bs[24..26].copy_from_slice(&32u16.to_le_bytes());            // BPB_SecPerTrk (cosmetic)
    bs[26..28].copy_from_slice(&2u16.to_le_bytes());             // BPB_NumHeads (cosmetic)
    bs[36] = 0x80;                                               // BS_DrvNum
    bs[38] = 0x29;                                               // BS_BootSig — VolID/VolLab present
    bs[39..43].copy_from_slice(&0x4C43_3443u32.to_le_bytes());   // BS_VolID ("LC4C") — the witness prints it
    bs[43..54].copy_from_slice(b"UNAOS SDHC4");                  // BS_VolLab (11 B, space padded)
    bs[54..62].copy_from_slice(b"FAT16   ");                     // BS_FilSysType
    bs[510] = 0x55; bs[511] = 0xAA;

    // ---- the FATs: cluster 2..(2+need-1) is ONE contiguous chain, then EOC ----
    let mut fat = vec![0u8; (SD4C_FAT_SECTORS as usize) * 512];
    let put = |f: &mut [u8], i: u32, v: u16| {
        let o = (i as usize) * 2;
        f[o..o + 2].copy_from_slice(&v.to_le_bytes());
    };
    put(&mut fat, 0, 0xFFF8); // media descriptor in entry 0
    put(&mut fat, 1, 0xFFFF); // EOC in entry 1
    for k in 0..need_clusters {
        let c = SD4C_FIRST_CLUSTER + k;
        let next = if k + 1 == need_clusters { 0xFFFF } else { (c + 1) as u16 };
        put(&mut fat, c, next);
    }
    for n in 0..SD4C_NUM_FATS {
        let off = ((SD4C_RESERVED + n * SD4C_FAT_SECTORS) as usize) * 512;
        img[off..off + fat.len()].copy_from_slice(&fat);
    }

    // ---- root directory: one 8.3 entry, no LFN ----
    let root_off = ((SD4C_RESERVED + SD4C_NUM_FATS * SD4C_FAT_SECTORS) as usize) * 512;
    let de = &mut img[root_off..root_off + 32];
    de[0..11].copy_from_slice(b"UNALOG  BIN"); // 8.3, space padded, no dot on disk
    de[11] = 0x20;                             // ATTR_ARCHIVE — a plain file
    de[26..28].copy_from_slice(&(SD4C_FIRST_CLUSTER as u16).to_le_bytes()); // DIR_FstClusLO
    de[28..32].copy_from_slice(&SD4C_RESERVE_BYTES.to_le_bytes());          // DIR_FileSize

    // ---- the reservation's own bytes: a recognisable fill, so a kernel write is VISIBLE ----
    // 0xAA is not 0x00: a host-side `xxd` after the run distinguishes "the kernel wrote here" from
    // "this sector was never touched", which zero-fill would not.
    let data_off = (extent_start as usize) * 512;
    let data_len = ((extent_end - extent_start) as usize) * 512;
    for b in &mut img[data_off..data_off + data_len] {
        *b = 0xAA;
    }

    std::fs::write(path, &img).expect("failed to write target/sdcard.img");
    println!(
        "   SDHC-4c: staged FAT16 card image ({}) — UNALOG.BIN cluster={} size={} contiguous, \
         data@LBA{} => PREDICTED reserved extent LBA [{}..{}) ({} sectors); the kernel's \
         `:: SDHC4C: reserve ...` witness must name exactly this",
        path.display(), SD4C_FIRST_CLUSTER, SD4C_RESERVE_BYTES, first_data_sector,
        extent_start, extent_end, extent_end - extent_start
    );
}

/// DEFAULTMEDIUM — build `builder/usb-boot.img`, the default x86 `test` stick, and return its path.
///
/// The long WHY is at the call site (search `DEFAULTMEDIUM:` in `main`). The short form: the default
/// medium has to answer BOTH questions the boot asks of it — the BOT fixture's raw signature at
/// LBA 0 bytes 0..21, and `fs::bootdisk`'s "does any mountable volume carry THIS kernel" — and an
/// MBR is the one sector layout where those two coexist without a kernel-side read-offset change.
///
/// GEOMETRY, and the witness each constant belongs to:
/// * `DISK_SECTORS` 131072 — `usbw`'s scratch is `num_blocks - 1`; the measured line is
///   `usbw. write lba=131071 ok` and this is the only size that keeps it.
/// * `PART_LBA` 2048 — 1 MiB alignment, the same start `make-fat-img.sh`'s `part` layout uses, so
///   the volume inside is byte-for-byte a volume any formatter would have produced.
/// * `FS_SECTORS` 126976 (62 MiB) — the partition ends at LBA 129024, BELOW 131071, so
///   `usbw_keepout_ceiling` reports `mbr-partition-table` with a ceiling the top-of-medium candidate
///   clears. A volume that ran to the last sector would make the probe skip instead of write.
///
/// Errors are returned, never panicked: a host without dosfstools/mtools must still be able to run
/// `./arroyo test` — it simply gets the old raw stick and the old two reds, said out loud.
fn build_default_medium(workspace_dir: &std::path::Path) -> Result<std::path::PathBuf, String> {
    use std::io::{Seek, SeekFrom, Write};

    const SECTOR: u64 = 512;
    const DISK_SECTORS: u64 = 131_072; // 64 MiB — pins `usbw. write lba=131071`
    const PART_LBA: u64 = 2_048;
    const FS_MB: u64 = 62;
    const FS_SECTORS: u64 = FS_MB * 2_048; // 126976 -> partition ends at LBA 129024
    const SIG: &[u8] = b"UNA-OS-DISK-001-ALPHA";
    const PART_TYPE_FAT32_LBA: u8 = 0x0c;

    let out = workspace_dir.join("builder/usb-boot.img");
    let fs_img = workspace_dir.join("builder/usb-boot.fs.img"); // `.img` so the repo `.gitignore` covers a leftover on failure
    let script = workspace_dir.join("scripts/make-fat-img.sh");
    if !script.exists() {
        return Err(format!("{} is missing", script.display()));
    }

    // The FAT32 payload, with no partition table of its own — `sf` is exactly that. `FAT_IMG_MB`
    // sizes it to the partition. `UNAOS_SRCFIXTURE` is REMOVED rather than left to the inherited
    // environment: the SELFHOST lane sets it, and a default stick whose root directory silently
    // grew two entries would move counts other fixtures assert.
    let status = Command::new("bash")
        .arg(&script)
        .arg("sf")
        .arg(&fs_img)
        .env("FAT_IMG_MB", FS_MB.to_string())
        .env_remove("UNAOS_SRCFIXTURE")
        .current_dir(workspace_dir)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| format!("could not run {}: {}", script.display(), e))?;
    if !status.success() {
        return Err(format!("{} sf exited with {}", script.display(), status));
    }

    // The payload must be EXACTLY the partition's length. `parse_bpb` refuses a volume whose own
    // `tot_sec` disagrees with the partition entry that pointed at it (fs/fat.rs, the GR9 gate), so a
    // mis-sized payload would mount nowhere — and it would do it silently, as a `NotFat`.
    let fs_len = std::fs::metadata(&fs_img)
        .map_err(|e| format!("cannot stat {}: {}", fs_img.display(), e))?
        .len();
    if fs_len != FS_SECTORS * SECTOR {
        return Err(format!(
            "{} is {} bytes, expected {} ({} sectors)",
            fs_img.display(), fs_len, FS_SECTORS * SECTOR, FS_SECTORS
        ));
    }

    let mut dst = std::fs::OpenOptions::new()
        .create(true).write(true).truncate(true).open(&out)
        .map_err(|e| format!("cannot create {}: {}", out.display(), e))?;
    dst.set_len(DISK_SECTORS * SECTOR)
        .map_err(|e| format!("cannot size {}: {}", out.display(), e))?;
    dst.seek(SeekFrom::Start(PART_LBA * SECTOR))
        .map_err(|e| format!("cannot seek {}: {}", out.display(), e))?;
    let mut src = std::fs::File::open(&fs_img)
        .map_err(|e| format!("cannot open {}: {}", fs_img.display(), e))?;
    std::io::copy(&mut src, &mut dst)
        .map_err(|e| format!("cannot place the volume in {}: {}", out.display(), e))?;

    // The MBR. Bytes 0..21 are the BOT fixture's pattern, sitting in the boot-code area; the table
    // starts at 446 and the signature word at 510, so nothing the kernel reads overlaps anything
    // else the kernel reads. The boot flag stays 0x00 — OVMF must keep booting the ide-hd ESP at
    // bootindex=0, and `make-fat-img.sh` has already renamed this volume's BOOTX64.EFI to .REM for
    // the same reason. The CHS triples are the usual 0xFE/0xFF/0xFF "past CHS, read the LBA fields"
    // sentinel; `block::decode_mbr` reads the LBA fields only and validates neither CHS nor the boot
    // flag (it checks type, extent and disjointness), so this entry is accepted as slot 1.
    let mut mbr = [0u8; SECTOR as usize];
    mbr[..SIG.len()].copy_from_slice(SIG);
    let ent = 446usize; // slot 1 of the primary partition table
    mbr[ent] = 0x00;
    mbr[ent + 1..ent + 4].copy_from_slice(&[0xfe, 0xff, 0xff]);
    mbr[ent + 4] = PART_TYPE_FAT32_LBA;
    mbr[ent + 5..ent + 8].copy_from_slice(&[0xfe, 0xff, 0xff]);
    mbr[ent + 8..ent + 12].copy_from_slice(&(PART_LBA as u32).to_le_bytes());
    mbr[ent + 12..ent + 16].copy_from_slice(&(FS_SECTORS as u32).to_le_bytes());
    mbr[510] = 0x55;
    mbr[511] = 0xaa;
    dst.seek(SeekFrom::Start(0))
        .map_err(|e| format!("cannot seek {}: {}", out.display(), e))?;
    dst.write_all(&mbr)
        .map_err(|e| format!("cannot write the MBR of {}: {}", out.display(), e))?;
    dst.sync_all()
        .map_err(|e| format!("cannot flush {}: {}", out.display(), e))?;
    let _ = std::fs::remove_file(&fs_img);
    Ok(out)
}
