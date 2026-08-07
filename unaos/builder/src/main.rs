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
    if std::env::var("UNAOS_MTRAW_INJECT").is_ok() { feats.push("mtraw_inject"); }
    else if std::env::var("UNAOS_MTRAW").is_ok() { feats.push("mtraw"); }
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
    // GR17 pay-as-you-go wc-g battery (video/wcg.rs): lattice-sampled first pass + deferred full
    // passes, x86-only paths, default OFF => byte-identical. Mapped here as well as in `arroyo`
    // for the same reason as BOTRING64 above: a knob arroyo alone sets never reaches boot media.
    if std::env::var("UNAOS_WCG_PAYGO").is_ok() { feats.push("wcg-paygo"); }
    // VPERF: x86 video-path bench instrumentation (scroll/VRAM-read counters, fbmem readout,
    // display-BAR probe, scripted scroll scenario). x86_64-only module; default OFF.
    if std::env::var("UNAOS_VIDEOBENCH").is_ok() { feats.push("videobench"); }
    // RAST-1: software-rasterizer spinning-cube demo through the x86/virt panel path. x86_64-only
    // knob; default OFF => byte-identical media (the `rast` dep + demo module are unlinked).
    if std::env::var("UNAOS_RAST").is_ok() { feats.push("rast"); }
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
    if std::env::var("UNAOS_SDW").is_ok() { feats.push("sdw"); }
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
    if std::env::var("UNAOS_KDISP_HOLD").is_ok() { feats.push("nvidia-kepler-kdisp-hold"); }
    // WC-X86: UNAOS_WC=1 arms the window compositor on the x86 panel path (video/wcx.rs) — activated
    // at the END of the Kepler takeover seam, after `fbcon::panel_console_resume`. x86_64-only
    // module; DEFAULT OFF => module + call site unlinked => byte-identical media. Needs
    // UNAOS_KEPLER + UNAOS_KEPLER_TAKEOVER to reach its seam. Kept in sync with arroyo's mapping.
    if std::env::var("UNAOS_WC").is_ok() { feats.push("wc"); }
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
    // GMUX-IGD: UNAOS_GMUX_IGD=1 arms the display-mux switch to the integrated GPU + timed
    // auto-revert. Kept in sync with arroyo's mapping — the builder rebuilds the kernel, so a knob
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
    // ping/arp/netinfo + socket syscalls + boot witnesses) UNLESS opted out with UNAOS_NOSMOLNET=1,
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
    // ORIN-SMP-DEFAULT: the real 6-core Orin SMP kick-off is DEFAULT-ON for tegra builds (opt out with
    // UNAOS_NOTEGRASMP=1). `tegrasmp` implies the aarch64 `tegra` board feature; this x86_64 builder
    // never produces aarch64 tegra media (arroyo's esp-jetson does, where the default-on lives), so this
    // maps the EXPLICIT UNAOS_TEGRASMP=1 knob for parity only. UNAOS_NOTEGRASMP is a no-op here (nothing
    // to suppress on x86 media). Kept in sync with arroyo's mapping.
    if std::env::var("UNAOS_TEGRASMP").is_ok() { feats.push("tegrasmp"); }
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
        println!("   U2: copied HELLO.BIN onto the ESP");
    } else {
        println!("   U2: target/hello.bin absent — ESP has no HELLO.BIN (run via ./arroyo esp-x86)");
    }

    // WINX-5: the x86 EL0 persistence program (crates/user-stat, built by arroyo's build_user_stat_x86
    // to target/STAT-X86.ELF). Copy it onto the ESP as STAT.ELF so the metal boot media carries it, the
    // same way HELLO.BIN reaches the volume just above; the x86 shell's `run`/`bg` read the FAT boot
    // partition's root, so `bg /fat/STAT.ELF` finds it there. The name is un-suffixed ON the volume
    // (STAT.ELF, not STAT-X86.ELF) because the operator command should read the same on both arches —
    // the arch suffix exists only in target/, where both arches' images share one directory.
    // Absent when the program wasn't built (a bare `cargo run` in builder/) — then `run`/`bg` simply
    // report -ENOENT, harmless.
    let stat_elf = target_dir.join("STAT-X86.ELF");
    if stat_elf.exists() {
        std::fs::copy(&stat_elf, esp_dir.join("STAT.ELF")).unwrap();
        println!("   WINX: copied STAT.ELF onto the ESP (bg /fat/STAT.ELF)");
    } else {
        println!("   WINX: target/STAT-X86.ELF absent — ESP has no STAT.ELF (run via ./arroyo esp-x86)");
    }

    // WINX-7: the x86 EL0 mini-vug (crates/user-vug, built by arroyo's build_user_vug_x86 to
    // target/VUG-X86.ELF), staged as VUG.ELF exactly like STAT.ELF above and for the same reasons —
    // un-suffixed on the volume so `bg /fat/VUG.ELF` reads the same on both arches.
    let vug_elf = target_dir.join("VUG-X86.ELF");
    if vug_elf.exists() {
        std::fs::copy(&vug_elf, esp_dir.join("VUG.ELF")).unwrap();
        println!("   WINX: copied VUG.ELF onto the ESP (bg /fat/VUG.ELF)");
    } else {
        println!("   WINX: target/VUG-X86.ELF absent — ESP has no VUG.ELF (run via ./arroyo esp-x86)");
    }

    // PULSE-1: the x86 EL0 cpu-pulse monitor (crates/user-pulse, built by arroyo's build_user_pulse_x86 to
    // target/PULSE-X86.ELF), staged as PULSE.ELF exactly like STAT.ELF/VUG.ELF above and for the same
    // reasons — un-suffixed on the volume so `bg /fat/PULSE.ELF` reads the same on both arches.
    let pulse_elf = target_dir.join("PULSE-X86.ELF");
    if pulse_elf.exists() {
        std::fs::copy(&pulse_elf, esp_dir.join("PULSE.ELF")).unwrap();
        println!("   PULSE: copied PULSE.ELF onto the ESP (bg /fat/PULSE.ELF)");
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
    // So `bg /fat/STAT.ELF` searches the USB stick while the build put STAT.ELF on the ESP. When the
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
    let mut staged_data: Vec<&str> = Vec::new();
    for (src, dst) in [
        (target_dir.join("hello.bin"), "HELLO.BIN"),
        (target_dir.join("STAT-X86.ELF"), "STAT.ELF"),
        (target_dir.join("VUG-X86.ELF"), "VUG.ELF"),
        (target_dir.join("PULSE-X86.ELF"), "PULSE.ELF"),
    ] {
        if src.exists() {
            std::fs::copy(&src, data_dir.join(dst)).unwrap();
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
        "   WINX-7 PKG: data volume tree target/x86_64_data/ — {} (+ hello.txt, readme.txt, SCRATCH.BIN, GROW.BIN, S8W.BIN, BLOCK.TXT)",
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

    // UNAOS_FATIMG selects a FAT filesystem image (built by scripts/make-fat-img.sh) as the
    // usb-storage backing instead of the raw UNA-OS pattern image, giving the kernel's read-only
    // FAT reader (`ls`/`cat`) a real FAT32 volume to parse. `1`/`part` -> builder/fat.img,
    // `sf` -> builder/fat-sf.img, or an explicit path. Unset (the default) keeps usb.img so the
    // BOT "MISSION SUCCESS" pattern test is unchanged. block::info() registers this single device,
    // mirroring a real single-stick metal boot where the FAT32 ESP stick *is* the block device.
    let stick_image = match std::env::var("UNAOS_FATIMG").ok().as_deref() {
        None | Some("") => usb_image.clone(),
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
    if stick_image != usb_image && !stick_image.exists() {
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
    cmd.arg("-machine").arg("pc-q35-10.0");

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
    // EHCI-1 scout (UNAOS_EHCISCOUT=1): give the read-only EHCI probe a QEMU target. q35's default
    // device set has no EHCI, so attach a standalone `usb-ehci` PCI controller (class 0x0C0320) — no
    // downstream device, so the scout reports the controller's cap/op/PORTSC state with 0 connected
    // ports (the honest QEMU result). This is a QEMU-harness knob, not a kernel write path.
    // EHCI-4 M1: the driver is DEFAULT-ON, so the harness usb-ehci controller + the usb-kbd-on-ehci
    // routing ride by default; UNAOS_NOEHCIHID=1 restores the pre-fold harness (kbd on xHCI, no EHCI
    // controller unless a scout knob asks for one).
    let ehcihid = std::env::var("UNAOS_NOEHCIHID").is_err();
    if std::env::var("UNAOS_EHCISCOUT").is_ok() || std::env::var("UNAOS_EHCICONFIG").is_ok() || ehcihid {
        cmd.arg("-device").arg("usb-ehci,id=ehci");
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
    // EHCI-3 harness: by default (EHCI-4 M1 driver on) the keyboard rides the EHCI bus (QEMU's usb-kbd is
    // HS-capable, so it trains directly on the EHCI root port — Topology B). It REPLACES the
    // xHCI keyboard in this mode so QMP `send-key` routes deterministically to the EHCI device
    // (two keyboards would leave the routing to QEMU's whim). QEMU cannot model the RMH hub
    // tier: its only hub is full-speed and wedges the machine at firmware if placed on the EHCI
    // bus — Topology A (hub walk + splits) is metal-first by construction.
    if ehcihid {
        cmd.arg("-device").arg("usb-kbd,bus=ehci.0");
    } else {
        cmd.arg("-device").arg("usb-kbd,bus=xhci.0");
    }
    // EHCI-4 M2 gate (UNAOS_EHCITABLET=1): move the usb-tablet onto the EHCI bus so the driver's
    // report-protocol POINTER path is exercised end-to-end — QEMU's usb-tablet is a non-boot
    // (proto 0) absolute pointer, exactly the trackpad shape, so the driver reads + parses its HID
    // report descriptor (GET_DESCRIPTOR(Report)), arms a report-protocol interrupt-IN QH, and
    // decodes X/Y/buttons to pal::Event::MouseAbsolute. Default: tablet stays on xHCI (the xHCI HID
    // tests keep their pointer). Only meaningful with the driver active (ignored under NOEHCIHID).
    let ehci_tablet = ehcihid && std::env::var("UNAOS_EHCITABLET").is_ok();
    if ehci_tablet {
        cmd.arg("-device").arg("usb-tablet,bus=ehci.0");
        println!("   UNAOS_EHCITABLET: usb-tablet on the EHCI bus — EHCI-4 M2 report-pointer path target");
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
    if usbserial {
        let ftdi_log = target_dir.join("ftdi.log");
        let _ = std::fs::remove_file(&ftdi_log); // start each run with a fresh capture file
        cmd.arg("-chardev").arg(format!("file,id=ftdi0,path={}", ftdi_log.display()))
           .arg("-device").arg("usb-serial,bus=xhci.0,chardev=ftdi0");
        println!("   U2.5: FTDI usb-serial attached; console capture -> {}", ftdi_log.display());
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
        if !sd_image.exists() {
            let f = std::fs::File::create(&sd_image).expect("failed to create target/sdcard.img");
            f.set_len(16 * 1024 * 1024).expect("failed to size target/sdcard.img");
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
        std::thread::sleep(std::time::Duration::from_secs(secs));
        let _ = child.kill();
        let _ = child.wait();
    } else {
        cmd.arg("-serial").arg("stdio");
        let mut child = cmd.spawn().unwrap();
        child.wait().unwrap();
    }
}
