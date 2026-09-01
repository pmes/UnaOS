// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// ORIN-NET-1 — read-only PCIe root-complex + NIC recon (`pcieprobe` gated) — the file's ORIGIN,
// NOT its scope. Two later arcs grew into this same module and BOTH write — NET-2 page-table
// descriptors, NET-3 fabric, config space, and the link-training enable. The split is stated in
// "## The READ/WRITE split" at the END of this header, and THAT section — not this line, and not
// the paragraph below — is the safety statement for the module.
//
// Orin has no network path yet. The Jetson Orin Nano devkit's NIC sits behind the Tegra234 PCIe
// root complex, so networking begins by knowing exactly what the firmware (NVIDIA UEFI / L4T
// 39.2.0) left us at ExitBootServices. The NET-1 CENSUS (`census`, `pcieprobe`) is the SMP-2-style
// *census-before-touch* that scopes the real bring-up chain (PCIe RC -> NIC -> smoltcp, already
// in-tree). THAT ENTRY POINT writes NOTHING to fabric or config space, enables no clock or power
// domain, retrains no link, and changes no power state — the wall (JETSON-XCARVE) taught this track
// to census before it touches. This sentence used to say "This module", and the split below records
// why that is struck.
//
// Two layers, in strict order of trust:
//
//   1. DTB census (ALWAYS, whenever this probe runs). Walk the firmware's OWN device tree READ-ONLY
//      and dump every `pcie@` controller: `reg` (+`reg-names`), `ranges`, interrupts, phys/lanes,
//      `power-domains`, and `status`. This is zero-MMIO-risk — the same bounded big-endian token
//      scan `fdt_tegra` already uses (a malformed blob degrades to a printed "not found", never a
//      fault). The census alone names which controllers exist, which the firmware left ENABLED
//      (`status = "okay"`), and — from the RC's child/`ranges` — where the NIC lives. This is the
//      deliverable's spine.
//
//   2. Config-space liveness read (GATED, conservative). ONLY for a controller the firmware left
//      ENABLED (`status = "okay"`) AND whose config/appl aperture lies inside the ALREADY-MAPPED
//      GiB-0 Device-nGnRE window (`mmu_tegra` maps GiB 0 + RAM; it does NOT map the high PCIe
//      config apertures). No new page-table write is performed (a mapping IS a write — the arc's
//      STOP tripwire). If the aperture is out of the mapped window, we record the blocker and leave
//      that controller UN-walked — a partial map with honest gaps beats a touched fabric; the
//      un-walked apertures become explicit NET-2 scope. Every read is plausibility-guarded and
//      behind a LIVENESS check that REJECTS poison patterns explicitly.
//
// ## The poison-rejection rule (PI-V3D-1, the cautionary tale)
//
// PI-V3D-1's attended Pi sitting found the V3D core block never decoded — every read returned the
// firmware's `0xdeadbeef` fill — yet the probe's liveness gate FALSE-PASSED it (it treated the
// non-zero word as "present"). A liveness gate that only rejects zero is not a liveness gate. Here,
// `is_poison()` rejects BOTH `0xffffffff` (the classic absent-decode / master-abort return) AND
// `0xdeadbeef` (firmware DRAM/register fill): either is ABSENT DECODE, never "present". A live
// config space returns a plausible vendor id (not 0x0000/0xffff) whose word is not a poison fill.
//
// ## The READ/WRITE split (corrected 2026-08-31 — the old claim was FALSE, not merely narrow)
//
// WHAT THIS SECTION REPLACES, quoted so the correction is RECORDED rather than silently restated
// (the `fs/sdhc4c.rs` and `sdmmc_tegra.rs` precedent). The header used to carry, under the title
// "Read-only invariant (the arc's review lens)": "Every access in this module is a `read_volatile`
// or a DTB byte read. There is no `write_volatile`, no `msr`/config write, no `SET_*` mailbox/MRQ,
// no `CPU_ON`, no link retrain, no BAR/command-reg write. ... The compiled module is gated
// `#[cfg(feature = "pcieprobe")]`."
//
// Every load-bearing clause of that is false. `write_volatile` appears TWICE — once directly in
// `net3_link_bringup`, once as the `write` closure in `net3_enumerate_and_size` that is invoked
// four times. The module ENABLES the LTSSM, which starts link training. It DOES write BARs. And the
// gate is wider than the sentence claims: `mod.rs` declares `pub mod pcie_probe;` under
// `#[cfg(any(feature = "pcieprobe", feature = "pcie2"))]`, with `pcie3 = ["pcie2"]` and
// `net4 = ["pcie3"]` above that. (Two clauses do survive and are kept below: no `msr`/mailbox/PSCI
// call and no command-register write exist anywhere in this file.)
//
// HOW LONG IT STOOD, and why this is a worse failure than a claim that merely aged. The paragraph
// was written in 223ddd53 (2026-07-17 16:13 -0600), when the module held nothing but `census`, and
// it was true as written. NET-3 (893fe5c7) falsified it at 21:44 the SAME DAY — 5 h 31 min later —
// by adding the exact primitive the sentence names as absent. No commit has touched these lines
// since: the header was byte-identical from 223ddd53 to this correction, 45 days. NET-3's own
// commit message calls itself "the lane's first deliberate fabric-write arc" and every write
// announces itself on the wire, so nothing was hidden — it simply never propagated to the first
// thing a reader meets. The absolute form is not the problem and stays keepable: `ga10b_probe.rs`
// makes the same "there is no `write_volatile` in this module" claim and it holds. Prose was the
// whole mechanism here, and prose does not fail a build.
//
// WHAT READS, with no MMIO write on any path: the NET-1 half in full (`census`, `decode_enabled`,
// `config_liveness_read`), the DTB/format helpers (`leaf`, `contains`, `stringlist_has`,
// `dump_words`, `dump_str_or_words`, `region_by_name`, `status_okay`), the liveness predicates
// (`is_poison`, `live_vendor_device`), `dbi_dll_active`, and `census2` itself up to the point where
// it hands off to `net2_link_and_device`.
//
// WHAT WRITES — three classes, in ascending order of consequence:
//   * PAGE-TABLE DESCRIPTORS (NET-2 M1). `report_map` inside `net2_link_and_device` calls
//     `mmu_tegra::map_mmio_window` for the dbi/config/ecam apertures (and appl, under `pcie3`);
//     `ps_widen_witness` calls it three more times. Each `Mapped` result installs a Device-nGnRE L1
//     block. The struck NET-1 text called a mapping "a write" and was right to.
//   * ONE FABRIC WRITE — the LTSSM ENABLE (NET-3 M2, `net3_link_bringup`): a read-modify-write of
//     APPL_CTRL setting LTSSM_EN (bit 7) on controller 0, followed by `dsb sy` and a
//     finite-backstop poll of DLL-active / RDLH. This is what the struck clause "no link retrain"
//     was reaching for, and it is STRONGER than a retrain: the PCIe-spec Retrain-Link bit
//     (LNKCTL[5]) is never written anywhere in this file, but enabling the DesignWare LTSSM takes
//     the link from not-training to training. In plain terms, this module brings a PCIe link UP.
//   * FOUR CONFIG-SPACE WRITES — the BAR-SIZING RITUAL (NET-3 M3, `net3_enumerate_and_size`): one
//     `write` closure invoked as all-ones probe + immediate restore on a BAR's low half, and the
//     same pair on a 64-bit BAR's high half. FIVE static MMIO write sites exist in this file — one
//     direct, four through that closure — counted here by enumeration rather than inherited;
//     dynamically the loop can issue up to twelve config writes across the six BAR slots.
//
// WHAT A READER MAY CONCLUDE, AND WHAT THEY MAY NOT. The bounding the original spirit promised does
// hold, and none of it is prose-only: the writes reach controller 0 and the one device below it,
// never another controller; config-space writes are confined to the BAR array (dword offsets
// 0x10..0x24) with an explicit guard that REFUSES a 64-bit type in BAR slot 5 rather than writing
// past the array into the Cardbus CIS pointer at 0x28; every BAR original is restored on the
// statement after its probe; each fabric write announces itself (">>> FABRIC WRITE") before issue;
// and no command register, no LNKCTL, no PERST, no PHY, no clock, no power domain, and no
// PSCI/mailbox call is written anywhere in this file. What a reader may NOT conclude is the thing
// the struck paragraph promised outright — that this module cannot bring a PCIe link up. Armed, it
// can, it does, and it leaves the link UP: nothing here tears it back down.
//
// REACHABILITY, so "carried" is never read as "runs". The module compiles under
// `any(pcieprobe, pcie2)`, reachable by any of `UNAOS_PCIEPROBE`, `UNAOS_PCIE2`, `UNAOS_PCIE3` or
// `UNAOS_NET4` (the last two imply `pcie2`). All are DEFAULT OFF, so a default jetson image does
// not compile this file at all and every artifact stays baseline-identical modulo the ratified
// 1-byte panic-Location class. The WRITING half additionally needs `tegra`: both `net3_*` functions
// are `#[cfg(all(feature = "pcie3", feature = "tegra"))]` and their only call site sits in a
// `pcie3` block inside `net2_link_and_device` (`all(pcie2, tegra)`), so a write requires
// `UNAOS_PCIE3=1 UNAOS_TEGRA=1` or `UNAOS_NET4=1 UNAOS_TEGRA=1`. That is not a hypothetical path:
// `net4` implies `pcie3`, so every Orin NETWORK image ever staged has carried the writers, and so
// will every future one. The one write path that does NOT need `tegra` is `ps_widen_witness`
// (`pcie3` alone) — on `virt` its `map_mmio_window` descriptors land in an inert static, which is
// why the QEMU witness is safe, not why the module is read-only. Independently of any media, all 27
// `arm-tegra*` legs of `./arroyo check` carry `pcie3,tegra`, so the writers are type-checked on
// every check run.
//
// This file is a natural adopter of the opt-in `// INVARIANT: no-mmio-writes` marker the SMMUHDR
// arc proposed: `census` / `config_liveness_read` could carry it; the two `net3_*` functions never
// could, and that asymmetry is exactly what the marker is for.

use super::fdt_tegra::Fdt;

/// Stable serial prefix so the operator (and `mbench`) can grep the whole census as one block.
const P: &str = ":: PCIE:";

/// The top of the `mmu_tegra` GiB-0 Device-nGnRE window (0x0..0x4000_0000). A config/appl aperture
/// below this is already mapped and readable at EL2; at or above it is UNMAPPED (the high Tegra234
/// PCIe config apertures live here) — reading it would need a page-table write, which the NET-1 census
/// will not do (STOP tripwire; NET-2 lifts this via `mmu_tegra::map_mmio_window`). See `mmu_tegra::init`
/// (`L1[0]` = the low-1-GiB Device window).
#[cfg(feature = "pcieprobe")]
const GIB0_DEVICE_TOP: u64 = 0x4000_0000;

/// Poison patterns that mean ABSENT DECODE, never "present" (PI-V3D-1 false-PASS lesson).
///  - `0xffffffff`: the PCIe master-abort / unclaimed-config return (no responder at that BDF).
///  - `0xdeadbeef`: firmware register/DRAM fill (the exact V3D false-PASS value).
// Used by the NET-1 config read (`pcieprobe`) and the NET-2 metal link/device read (`pcie2`+`tegra`);
// a `pcie2`-standalone virt-witness build does no MMIO, so it is compiled out there.
#[cfg(any(feature = "pcieprobe", all(feature = "pcie2", feature = "tegra")))]
#[inline]
fn is_poison(v: u32) -> bool {
    v == 0xffff_ffff || v == 0xdead_beef
}

/// A config-space `vendor:device` word is LIVE only if it is neither a poison fill nor the two
/// all-decode-boundary values (`0x0000_0000` = powered-off/no responder, `0xffff_????` vendor =
/// unclaimed). Returns `Some((vendor, device))` on a live decode, `None` on absent decode.
#[cfg(any(feature = "pcieprobe", all(feature = "pcie2", feature = "tegra")))]
#[inline]
fn live_vendor_device(w: u32) -> Option<(u16, u16)> {
    if is_poison(w) {
        return None;
    }
    let vendor = (w & 0xffff) as u16;
    let device = (w >> 16) as u16;
    // vendor 0x0000 (no power / no responder) and 0xffff (unclaimed) are both ABSENT.
    if vendor == 0x0000 || vendor == 0xffff {
        return None;
    }
    Some((vendor, device))
}

/// Context mirrors the tegra probe convention (`smpprobe::ProbeCtx`): the live firmware DTB + the
/// `mmu_tegra` RAM-GiB map (so a DTB in an unmapped GiB is rejected before any deref).
pub struct PcieCtx {
    pub dtb_addr: u64,
    pub dtb_size: usize,
    /// Bit `i` set <=> GiB `i` is mapped Normal-WB RAM. GiB 0 is always the Device window.
    pub ram_gib_mask: u64,
}

/// Is a GiB index reachable at EL2 without a new mapping? GiB 0 is the Device window (always), the
/// rest only if `mmu_tegra` marked it RAM. (Config apertures never live in RAM, so in practice only
/// GiB 0 passes for an MMIO config read — this is deliberately strict.)
#[inline]
fn gib_mapped(gib: u64, ram_gib_mask: u64) -> bool {
    gib == 0 || (gib < 64 && (ram_gib_mask >> gib) & 1 != 0)
}

/// The leaf name of a '/'-joined DTB path (everything after the last '/').
fn leaf(path: &[u8]) -> &[u8] {
    match path.iter().rposition(|&b| b == b'/') {
        Some(i) => &path[i + 1..],
        None => path,
    }
}

/// Does `hay` contain `needle` (bounded substring scan; DTB props are tiny)?
fn contains(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > hay.len() {
        return needle.is_empty();
    }
    hay.windows(needle.len()).any(|w| w == needle)
}

/// A DTB `stringlist` value (NUL-separated) contains `s`?
fn stringlist_has(val: &[u8], s: &[u8]) -> bool {
    val.split(|&b| b == 0).any(|item| item == s)
}

/// Print a raw prop value as big-endian u32 words (the DTB cell encoding), truncating loudly.
fn dump_words(name: &str, val: &[u8]) {
    // A stable, bounded dump: up to 16 cells, then "…(+N cells)".
    let cells = val.len() / 4;
    let shown = cells.min(16);
    // Build into a small stack string via repeated prints (no heap; this is a boot diagnostic).
    // One line: "<name> = <c0> <c1> … (Ncells[, LEN bytes])".
    // We can't format a variable-length list in one `serial_println!` without alloc, so emit the
    // header, then print the cells one per line.
    serial_println!("{} {} = [{} cells, {} bytes]", P, name, cells, val.len());
    let mut i = 0;
    while i < shown {
        let o = i * 4;
        let w = u32::from_be_bytes([val[o], val[o + 1], val[o + 2], val[o + 3]]);
        serial_println!("{}   [{}] {:#010x}", P, i, w);
        i += 1;
    }
    if cells > shown {
        serial_println!("{}   …(+{} more cells)", P, cells - shown);
    }
}

/// Print a prop value as a string if it is printable-ish (status/compatible/reg-names), else words.
fn dump_str_or_words(name: &str, val: &[u8]) {
    let printable = !val.is_empty()
        && val
            .iter()
            .all(|&b| b == 0 || (0x20..0x7f).contains(&b));
    if printable {
        // Replace NULs with '|' for stringlists; strip a trailing NUL.
        let mut buf = [0u8; 96];
        let n = val.len().min(buf.len());
        for (d, &s) in buf[..n].iter_mut().zip(&val[..n]) {
            *d = if s == 0 { b'|' } else { s };
        }
        // Trim a trailing '|'.
        let mut end = n;
        while end > 0 && buf[end - 1] == b'|' {
            end -= 1;
        }
        match core::str::from_utf8(&buf[..end]) {
            Ok(s) => serial_println!("{} {} = \"{}\"", P, name, s),
            Err(_) => dump_words(name, val),
        }
    } else {
        dump_words(name, val);
    }
}

/// The interesting props of a `pcie@` controller node, as raw byte slices captured in one walk.
struct CtrlProps<'a> {
    compatible: Option<&'a [u8]>,
    device_type: Option<&'a [u8]>,
    status: Option<&'a [u8]>,
    reg: Option<&'a [u8]>,
    reg_names: Option<&'a [u8]>,
    ranges: Option<&'a [u8]>,
    interrupt_map: Option<&'a [u8]>,
    interrupts: Option<&'a [u8]>,
    num_lanes: Option<&'a [u8]>,
    phy_names: Option<&'a [u8]>,
    power_domains: Option<&'a [u8]>,
    linux_pci_domain: Option<&'a [u8]>,
}

impl<'a> CtrlProps<'a> {
    const fn empty() -> Self {
        CtrlProps {
            compatible: None,
            device_type: None,
            status: None,
            reg: None,
            reg_names: None,
            ranges: None,
            interrupt_map: None,
            interrupts: None,
            num_lanes: None,
            phy_names: None,
            power_domains: None,
            linux_pci_domain: None,
        }
    }
}

/// Firmware-left state (`status = "okay"` and the config aperture) needed to gate the config read.
#[cfg(feature = "pcieprobe")]
struct EnabledDecode {
    okay: bool,
    /// The base of the `reg` region named "config" (or the DW `dbi` fallback), if resolvable.
    config_base: Option<u64>,
    config_kind: &'static str,
}

/// Resolve whether the firmware left this controller ENABLED and where its config aperture sits.
/// READ-ONLY: `status` string test + a `reg`/`reg-names` index lookup. Never touches MMIO.
#[cfg(feature = "pcieprobe")]
fn decode_enabled(p: &CtrlProps<'_>) -> EnabledDecode {
    // status absent defaults to "okay" per the DT spec, but for a bring-up census we are
    // conservative: only an explicit "okay" (or absent) counts as enabled; "disabled"/anything else
    // is treated as NOT enabled (no config touch).
    let okay = match p.status {
        None => true,
        Some(s) => stringlist_has(s, b"okay") || stringlist_has(s, b"ok"),
    };

    // reg is (addr:2 cells, size:2 cells) per region on Tegra /bus@0 children. reg-names labels each
    // region. Find "config" (the ECAM-like downstream config aperture); fall back to "dbi"/"appl"
    // (the RC's own register/config aperture) so link state can be read at GiB-0 when config is high.
    let mut config_base = None;
    let mut config_kind = "none";
    if let (Some(reg), Some(names)) = (p.reg, p.reg_names) {
        let region_base = |idx: usize| -> Option<u64> {
            let off = idx * 16; // 4 cells * 4 bytes
            let b = reg.get(off..off + 8)?;
            let hi = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as u64;
            let lo = u32::from_be_bytes([b[4], b[5], b[6], b[7]]) as u64;
            Some((hi << 32) | lo)
        };
        // Walk reg-names in order; remember the index of the first "config", else "dbi"/"appl".
        let mut idx = 0usize;
        let mut chosen: Option<(usize, &'static str)> = None;
        for item in names.split(|&b| b == 0) {
            if item.is_empty() {
                continue;
            }
            let kind = if item == b"config" {
                Some("config")
            } else if item == b"dbi" {
                Some("dbi")
            } else if item == b"appl" {
                Some("appl")
            } else {
                None
            };
            if let Some(k) = kind {
                // Prefer "config"; otherwise keep the first dbi/appl seen.
                match chosen {
                    Some((_, "config")) => {}
                    _ if k == "config" => chosen = Some((idx, k)),
                    None => chosen = Some((idx, k)),
                    _ => {}
                }
            }
            idx += 1;
        }
        if let Some((ci, k)) = chosen {
            config_base = region_base(ci);
            config_kind = k;
        }
    }

    EnabledDecode { okay, config_base, config_kind }
}

/// Guarded, poison-rejecting liveness read of a config/appl aperture that is provably in the mapped
/// GiB-0 Device window. Reads BDF (0,0,0)'s vendor/device, then class/rev, header type, and BAR0..5
/// — all READ-ONLY, no config write. On absent decode (poison / no responder) it says so and stops.
///
/// Safety: `base` MUST have passed `gib_mapped(base>>30, ..)==true` (GiB-0 device window) — the
/// caller enforces this. We still only read; a gated block would fault EL3-fatal (the JX1 lesson),
/// which is why the caller gates on the firmware's own `status = "okay"` (the enable oracle) first.
#[cfg(feature = "pcieprobe")]
fn config_liveness_read(base: u64, kind: &str) {
    // BDF(0,0,0) offset 0 = [device:16 | vendor:16].
    let vd = unsafe { core::ptr::read_volatile(base as *const u32) };
    serial_println!("{}   {} @ {:#x}: cfg[0x00] = {:#010x}", P, kind, base, vd);
    let (vendor, device) = match live_vendor_device(vd) {
        None => {
            serial_println!(
                "{}   -> ABSENT DECODE (poison/unclaimed) — NOT present (PI-V3D-1 rule) ::",
                P
            );
            return;
        }
        Some(vd) => vd,
    };
    serial_println!(
        "{}   -> LIVE: vendor={:#06x} device={:#06x} ::",
        P,
        vendor,
        device
    );
    // Class/rev at offset 0x08 = [class:8 | subclass:8 | prog-if:8 | rev:8].
    let cr = unsafe { core::ptr::read_volatile((base + 0x08) as *const u32) };
    if is_poison(cr) {
        serial_println!("{}   cfg[0x08] = {:#010x} (poison — class unread) ::", P, cr);
    } else {
        serial_println!(
            "{}   class={:#04x} subclass={:#04x} progif={:#04x} rev={:#04x} ::",
            P,
            (cr >> 24) & 0xff,
            (cr >> 16) & 0xff,
            (cr >> 8) & 0xff,
            cr & 0xff
        );
    }
    // Header type at 0x0c bits [23:16].
    let ht = unsafe { core::ptr::read_volatile((base + 0x0c) as *const u32) };
    if !is_poison(ht) {
        serial_println!("{}   header-type={:#04x} ::", P, (ht >> 16) & 0xff);
    }
    // BAR0..5 (read-only decode of the firmware-left values; we NEVER write a BAR).
    let mut bar = 0x10u64;
    let mut i = 0;
    while i < 6 {
        let v = unsafe { core::ptr::read_volatile((base + bar) as *const u32) };
        // A BAR reading poison-fill is unimplemented; 0 is disabled. Report raw, no interpretation
        // that could mask an absent decode.
        serial_println!("{}   BAR{} [{:#04x}] = {:#010x}{} ::", P, i, bar, v,
            if is_poison(v) { " (poison/unimpl)" } else { "" });
        bar += 4;
        i += 1;
    }
    serial_println!(
        "{}   link-state / cap-walk = ATTENDED-METAL-PENDING (NET-2: read LTSSM via appl/DBI PCIE cap) ::",
        P
    );
}

/// The census entry point. Walk the DTB, dump every `pcie@` controller, and — for enabled
/// controllers whose config aperture is already mapped — do a guarded, poison-rejecting liveness
/// read. Graceful on any missing/foreign DTB (QEMU `virt` has a generic ecam, no Tegra234 RC).
#[cfg(feature = "pcieprobe")]
pub fn census(ctx: &PcieCtx) {
    serial_println!(
        "{} ORIN-NET-1 read-only PCIe/NIC census (DTB @{:#x} size={:#x}) ::",
        P,
        ctx.dtb_addr,
        ctx.dtb_size
    );
    if ctx.dtb_addr == 0 || ctx.dtb_size == 0 {
        serial_println!("{} no DTB handed off — census SKIPPED (graceful) ::", P);
        return;
    }
    let g_lo = ctx.dtb_addr >> 30;
    let g_hi = (ctx.dtb_addr + ctx.dtb_size as u64 - 1) >> 30;
    if !gib_mapped(g_lo, ctx.ram_gib_mask) || !gib_mapped(g_hi, ctx.ram_gib_mask) {
        serial_println!("{} DTB in an unmapped GiB — census SKIPPED (graceful) ::", P);
        return;
    }
    let blob = unsafe { core::slice::from_raw_parts(ctx.dtb_addr as *const u8, ctx.dtb_size) };
    let Some(fdt) = Fdt::new(blob) else {
        serial_println!("{} DTB header invalid — census SKIPPED (graceful) ::", P);
        return;
    };

    // Pass 1: collect the '/'-joined paths of every `pcie@…` controller node (deduped). Bounded by
    // MAX_CTRL; a devkit has <= a handful of PCIe controllers.
    const MAX_CTRL: usize = 8;
    const PATH_CAP: usize = 160;
    let mut paths = [[0u8; PATH_CAP]; MAX_CTRL];
    let mut plens = [0usize; MAX_CTRL];
    let mut n_ctrl = 0usize;
    fdt.for_each_prop(|e| {
        if n_ctrl >= MAX_CTRL {
            return;
        }
        if !leaf(e.path).starts_with(b"pcie@") {
            return;
        }
        // dedupe: same as the last recorded path?
        if n_ctrl > 0 && &paths[n_ctrl - 1][..plens[n_ctrl - 1]] == e.path {
            return;
        }
        // also skip if it matches ANY recorded (props of a node are consecutive, but be safe).
        for k in 0..n_ctrl {
            if &paths[k][..plens[k]] == e.path {
                return;
            }
        }
        let l = e.path.len().min(PATH_CAP);
        paths[n_ctrl][..l].copy_from_slice(&e.path[..l]);
        plens[n_ctrl] = l;
        n_ctrl += 1;
    });

    if n_ctrl == 0 {
        serial_println!(
            "{} no `pcie@` controllers in the DTB — no Tegra234 PCIe RC (graceful; QEMU virt / no-net) ::",
            P
        );
        return;
    }
    serial_println!("{} {} PCIe controller node(s) found ::", P, n_ctrl);

    // Pass 2: per controller, one walk that captures the interesting props by raw byte slice, dumps
    // them, then gates the config-space liveness read.
    for c in 0..n_ctrl {
        let path = &paths[c][..plens[c]];
        serial_println!("{} ── controller {}: /{} ──", P, c,
            core::str::from_utf8(&path[1..]).unwrap_or("<non-utf8>"));
        let mut props = CtrlProps::empty();
        fdt.for_each_prop(|e| {
            if e.path != path {
                return;
            }
            let val = &blob[e.val_off..e.val_off + e.val_len];
            match e.name {
                b"compatible" => props.compatible = Some(val),
                b"device_type" => props.device_type = Some(val),
                b"status" => props.status = Some(val),
                b"reg" => props.reg = Some(val),
                b"reg-names" => props.reg_names = Some(val),
                b"ranges" => props.ranges = Some(val),
                b"interrupt-map" => props.interrupt_map = Some(val),
                b"interrupts" => props.interrupts = Some(val),
                b"num-lanes" => props.num_lanes = Some(val),
                b"phy-names" => props.phy_names = Some(val),
                b"power-domains" => props.power_domains = Some(val),
                b"linux,pci-domain" => props.linux_pci_domain = Some(val),
                _ => {}
            }
        });

        if let Some(v) = props.compatible {
            dump_str_or_words("compatible", v);
        }
        if let Some(v) = props.device_type {
            dump_str_or_words("device_type", v);
        }
        if let Some(v) = props.linux_pci_domain {
            dump_words("linux,pci-domain", v);
        }
        if let Some(v) = props.status {
            dump_str_or_words("status", v);
        } else {
            serial_println!("{}   status = (absent => \"okay\" per DT spec) ::", P);
        }
        if let Some(v) = props.reg_names {
            dump_str_or_words("reg-names", v);
        }
        if let Some(v) = props.reg {
            dump_words("reg", v);
        }
        if let Some(v) = props.ranges {
            dump_words("ranges", v);
        }
        if let Some(v) = props.num_lanes {
            dump_words("num-lanes", v);
        }
        if let Some(v) = props.phy_names {
            dump_str_or_words("phy-names", v);
        }
        if let Some(v) = props.power_domains {
            dump_words("power-domains", v);
        }
        if let Some(v) = props.interrupts {
            dump_words("interrupts", v);
        }
        if let Some(v) = props.interrupt_map {
            // interrupt-map is large; the count alone scopes the legacy-INTx routing for NET-2.
            serial_println!("{}   interrupt-map = present ({} bytes) ::", P, v.len());
        }

        // Is this a Tegra234 DesignWare RC (vs a generic ecam / foreign controller)?
        let is_tegra_rc = props
            .compatible
            .map(|c| contains(c, b"tegra234-pcie") || contains(c, b"tegra194-pcie") || contains(c, b"snps,dw-pcie"))
            .unwrap_or(false);

        // ── Config-space liveness gate ──
        let dec = decode_enabled(&props);
        serial_println!(
            "{}   enabled(firmware)={} tegra-RC={} ::",
            P,
            dec.okay,
            is_tegra_rc
        );
        if !dec.okay {
            serial_println!("{}   config walk SKIPPED — firmware left it DISABLED ::", P);
            continue;
        }
        let Some(base) = dec.config_base else {
            serial_println!(
                "{}   config walk SKIPPED — no config/dbi/appl reg region resolvable from DTB ::",
                P
            );
            continue;
        };
        let gib = base >> 30;
        if base >= GIB0_DEVICE_TOP || !gib_mapped(gib, ctx.ram_gib_mask) {
            // The honest gap: reading here needs a page-table write to map the aperture, which this
            // read-only arc will NOT do (STOP tripwire). Record the blocker; NET-2 maps it.
            serial_println!(
                "{}   config walk BLOCKED — {} aperture {:#x} (GiB {}) is OUTSIDE the mapped GiB-0 device window; NET-2 must map it Device-nGnRE first (no write here) ::",
                P,
                dec.config_kind,
                base,
                gib
            );
            continue;
        }
        serial_println!(
            "{}   config walk: {} aperture {:#x} is in the mapped GiB-0 window — guarded read ::",
            P,
            dec.config_kind,
            base
        );
        config_liveness_read(base, dec.config_kind);
    }

    serial_println!("{} ORIN-NET-1 census DONE (read-only; metal columns attended-pending) ::", P);
}

// ══════════════════════════════════════════════════════════════════════════════════════════════════
// ORIN-NET-2 — controller-0 link state + device enumeration (the `pcie2` feature).
// ══════════════════════════════════════════════════════════════════════════════════════════════════
//
// NET-1's census named controller 0 (`/bus@0/pcie@140a0000`, domain 8) firmware-ENABLED with a full
// `appl|config|atu_dma|dbi|ecam` reg map, then read the DOWNSTREAM `config` window (0x2a00_0000) and got
// `0xffffffff` — an ABSENT DECODE meaning "nothing is answering below the root port", i.e. the link is
// most likely DOWN. NET-2 answers the two questions that scope the real driver arc (NET-3): (1) is the
// link up, and (2) WHAT DEVICE is behind it. Read-only, with ONE class of write permitted — kernel
// page-table mappings — and poison-rejecting liveness on every read.
//
// **The NET-1 lesson corrected.** The downstream `config` window routes to the first downstream bus via
// the controller's internal ATU and returns all-Fs when the link is down (or the CFG ATU region is
// unset). The ROOT PORT'S OWN identity and LINK STATE do NOT live there — they live in the `dbi`
// aperture (0x2a08_0000), the DesignWare RP's local config space, valid regardless of link state. So
// NET-2 reads link state from DBI (the PCIe-capability Link Status register), not the downstream window.
//
// **The aperture map (metal census, r21b sitting).** appl `0x140a_0000`, config `0x2a00_0000`,
// atu_dma `0x2a04_0000`, dbi `0x2a08_0000` all sit in the already-mapped GiB-0 device window; the
// `ecam` whole-domain enumeration window (Tegra234: `0x2e_2000_0000`, 256 MiB) and the MMIO `ranges`
// (`0x32_/0x35_…`, ~200 GiB) live ABOVE the tegra regime's 36-bit PS ceiling (64 GiB). Reaching the
// ECAM for a full multi-bus enumeration is therefore the concrete NET-3 blocker `map_mmio_window`
// reports (it needs a TCR_EL2.PS widen to 40-bit — a translation-regime change beyond a page-table
// write, which a read-only recon arc records rather than performs). The scoped NET-2 walk (RP link
// state via DBI + one level below via the already-mapped `config` window) needs no beyond-ceiling map.

/// NET-2 serial prefix (still matches an `awk '/PCIE/'` sweep, but tags the link/device verdict as its
/// own sub-block so the operator can separate it from the NET-1 census dump). The shared `dump_*`
/// formatters keep the `:: PCIE:` prefix; only NET-2's own verdict lines carry `:: PCIE2:`.
#[cfg(feature = "pcie2")]
const P2: &str = ":: PCIE2:";

/// ORIN-NET-3 serial prefix (M1 PS-widen witness + M2/M3 link-bring-up/enumeration verdicts).
#[cfg(feature = "pcie3")]
const P3: &str = ":: PCIE3:";

/// ORIN-NET-3 (M1) QEMU witness — runs on the `virt` GICv3 path (the only PCIe surface QEMU offers,
/// since it models no Tegra234 RC). It cannot exercise the *actual* TCR widen (that regime is only
/// programmed on the metal tegra boot), but it CAN witness the thing the widen changes: the ceiling
/// decision inside `map_mmio_window`. It INVERTS the NET-2 regression witness — the controller-0 ECAM
/// (`0x2e_2000_0000`, ~184 GiB) that NET-2's `map_mmio_window` REFUSED (`BeyondPsCeiling`, 36-bit
/// ceiling) must now be REACHABLE — and asserts the refusal path still triggers for a base beyond the
/// reachable range (the 512-GiB table extent, and above the 40-bit output ceiling). On `virt` the
/// `mmu_tegra` L1 tables are NOT the active regime (the boot core translates through `boot_virt`'s
/// table), so the descriptor `map_mmio_window` writes into the inert static is functionally invisible
/// here — the witness observes only the returned reach classification. Prints a single PASS/FAIL line
/// the harness greps. Gated `pcie3`; compiled out (and the call site vanishes) knob-off.
#[cfg(feature = "pcie3")]
pub fn ps_widen_witness() {
    use super::mmu_tegra::{map_mmio_window, MmioMap};
    serial_println!(
        "{} ORIN-NET-3 PS-widen mapping witness (QEMU virt; the tegra TCR widen itself is metal-only) ::",
        P3
    );
    // Controller-0 ECAM: 0x2e_2000_0000 (~184 GiB), 256 MiB — the aperture NET-2 refused at 36-bit.
    const ECAM_BASE: u64 = 0x2e_2000_0000;
    const ECAM_SIZE: usize = 256 * 1024 * 1024;
    let ecam_reach = map_mmio_window(ECAM_BASE, ECAM_SIZE);
    let ecam_ok = matches!(ecam_reach, MmioMap::Mapped | MmioMap::AlreadyMapped);
    serial_println!(
        "{}   ECAM {:#x} (+{:#x}, GiB {}): {} ::",
        P3,
        ECAM_BASE,
        ECAM_SIZE,
        ECAM_BASE >> 30,
        if ecam_ok {
            "REACHABLE (NET-2 BeyondPsCeiling refusal INVERTED by the 40-bit widen)"
        } else {
            "still refused (widen NOT in effect)"
        }
    );
    // Refusal must persist above the reachable range: at the 512-GiB L1 table extent, AND above the
    // 40-bit (1 TiB) output ceiling. Both must return BeyondPsCeiling (and neither may index the table).
    const AT_TABLE_EXTENT: u64 = 512u64 << 30; // 512 GiB — the 512-entry L1 table's VA reach
    const ABOVE_40BIT: u64 = 1024u64 << 30; // 1 TiB — above the widened 40-bit output ceiling
    let refuse_extent = matches!(map_mmio_window(AT_TABLE_EXTENT, 0x1000), MmioMap::BeyondPsCeiling);
    let refuse_40bit = matches!(map_mmio_window(ABOVE_40BIT, 0x1000), MmioMap::BeyondPsCeiling);
    serial_println!(
        "{}   refusal preserved: @512GiB(table-extent)={} @1TiB(>40-bit)={} ::",
        P3,
        refuse_extent,
        refuse_40bit
    );
    if ecam_ok && refuse_extent && refuse_40bit {
        serial_println!("{} ORIN-NET-3 PS-widen witness: PASS ::", P3);
    } else {
        serial_println!("{} ORIN-NET-3 PS-widen witness: FAIL ::", P3);
    }
}

/// Read a controller's `status` the NET-1 way, standalone (no `EnabledDecode`): absent or "okay"/"ok"
/// ⇒ enabled; "disabled"/anything else ⇒ NOT enabled (no MMIO touch on a disabled controller).
#[cfg(feature = "pcie2")]
fn status_okay(p: &CtrlProps<'_>) -> bool {
    match p.status {
        None => true,
        Some(s) => stringlist_has(s, b"okay") || stringlist_has(s, b"ok"),
    }
}

/// Resolve the `[base, size)` of the `reg` region named `want` (e.g. `b"dbi"`) by walking `reg-names`
/// in order and indexing `reg` (4 cells = addr:2 + size:2 per region, big-endian). READ-ONLY DTB decode.
#[cfg(feature = "pcie2")]
fn region_by_name(p: &CtrlProps<'_>, want: &[u8]) -> Option<(u64, u64)> {
    let (reg, names) = (p.reg?, p.reg_names?);
    let mut idx = 0usize;
    for item in names.split(|&b| b == 0) {
        if item.is_empty() {
            continue;
        }
        if item == want {
            let off = idx * 16;
            let b = reg.get(off..off + 16)?;
            let a_hi = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as u64;
            let a_lo = u32::from_be_bytes([b[4], b[5], b[6], b[7]]) as u64;
            let s_hi = u32::from_be_bytes([b[8], b[9], b[10], b[11]]) as u64;
            let s_lo = u32::from_be_bytes([b[12], b[13], b[14], b[15]]) as u64;
            return Some(((a_hi << 32) | a_lo, (s_hi << 32) | s_lo));
        }
        idx += 1;
    }
    None
}

/// ORIN-NET-2 entry point. Focuses on CONTROLLER 0 (the first `pcie@` node — `pcie@140a0000` on the
/// Orin devkit): dump its DTB reg map, reach its config/ECAM aperture via the kernel page-table path,
/// read link state from DBI, and — if the link is up — walk bus 0 dev 0 and one level below. Graceful
/// on any missing/foreign DTB (the QEMU `virt` witness: `dtb_addr=0` on the GICv3 path, or a generic
/// ecam with no Tegra234 RC ⇒ skip before any MMIO).
#[cfg(feature = "pcie2")]
pub fn census2(ctx: &PcieCtx) {
    serial_println!(
        "{} ORIN-NET-2 controller-0 link + device recon (DTB @{:#x} size={:#x}) ::",
        P2, ctx.dtb_addr, ctx.dtb_size
    );
    if ctx.dtb_addr == 0 || ctx.dtb_size == 0 {
        serial_println!("{} no DTB handed off — recon SKIPPED (graceful) ::", P2);
        return;
    }
    let g_lo = ctx.dtb_addr >> 30;
    let g_hi = (ctx.dtb_addr + ctx.dtb_size as u64 - 1) >> 30;
    if !gib_mapped(g_lo, ctx.ram_gib_mask) || !gib_mapped(g_hi, ctx.ram_gib_mask) {
        serial_println!("{} DTB in an unmapped GiB — recon SKIPPED (graceful) ::", P2);
        return;
    }
    let blob = unsafe { core::slice::from_raw_parts(ctx.dtb_addr as *const u8, ctx.dtb_size) };
    let Some(fdt) = Fdt::new(blob) else {
        serial_println!("{} DTB header invalid — recon SKIPPED (graceful) ::", P2);
        return;
    };

    // Controller 0 = the FIRST `pcie@` node (dedup not needed — we stop at the first).
    const PATH_CAP: usize = 160;
    let mut path0 = [0u8; PATH_CAP];
    let mut plen0 = 0usize;
    let mut found = false;
    fdt.for_each_prop(|e| {
        if found || !leaf(e.path).starts_with(b"pcie@") {
            return;
        }
        let l = e.path.len().min(PATH_CAP);
        path0[..l].copy_from_slice(&e.path[..l]);
        plen0 = l;
        found = true;
    });
    if !found {
        serial_println!(
            "{} no `pcie@` controllers in the DTB — no Tegra234 PCIe RC (graceful; QEMU virt / no-net) ::",
            P2
        );
        return;
    }
    let path = &path0[..plen0];
    serial_println!(
        "{} controller 0: /{} ::",
        P2,
        core::str::from_utf8(&path[1..]).unwrap_or("<non-utf8>")
    );

    // Capture its props in one walk (full dump, mirroring the NET-1 census fields).
    let mut props = CtrlProps::empty();
    fdt.for_each_prop(|e| {
        if e.path != path {
            return;
        }
        let val = &blob[e.val_off..e.val_off + e.val_len];
        match e.name {
            b"compatible" => props.compatible = Some(val),
            b"device_type" => props.device_type = Some(val),
            b"status" => props.status = Some(val),
            b"reg" => props.reg = Some(val),
            b"reg-names" => props.reg_names = Some(val),
            b"ranges" => props.ranges = Some(val),
            b"interrupt-map" => props.interrupt_map = Some(val),
            b"interrupts" => props.interrupts = Some(val),
            b"num-lanes" => props.num_lanes = Some(val),
            b"phy-names" => props.phy_names = Some(val),
            b"power-domains" => props.power_domains = Some(val),
            b"linux,pci-domain" => props.linux_pci_domain = Some(val),
            _ => {}
        }
    });

    if let Some(v) = props.compatible {
        dump_str_or_words("compatible", v);
    }
    if let Some(v) = props.device_type {
        dump_str_or_words("device_type", v);
    }
    if let Some(v) = props.linux_pci_domain {
        dump_words("linux,pci-domain", v);
    }
    if let Some(v) = props.status {
        dump_str_or_words("status", v);
    } else {
        serial_println!("{}   status = (absent => \"okay\" per DT spec) ::", P2);
    }
    if let Some(v) = props.reg_names {
        dump_str_or_words("reg-names", v);
    }
    if let Some(v) = props.reg {
        dump_words("reg", v);
    }
    if let Some(v) = props.ranges {
        dump_words("ranges", v);
    }
    if let Some(v) = props.num_lanes {
        dump_words("num-lanes", v);
    }
    if let Some(v) = props.phy_names {
        dump_str_or_words("phy-names", v);
    }
    if let Some(v) = props.power_domains {
        dump_words("power-domains", v);
    }
    if let Some(v) = props.interrupts {
        dump_words("interrupts", v);
    }
    if let Some(v) = props.interrupt_map {
        serial_println!("{}   interrupt-map = present ({} bytes) ::", P2, v.len());
    }

    let is_tegra_rc = props
        .compatible
        .map(|c| {
            contains(c, b"tegra234-pcie")
                || contains(c, b"tegra194-pcie")
                || contains(c, b"snps,dw-pcie")
        })
        .unwrap_or(false);
    let okay = status_okay(&props);
    serial_println!("{}   enabled(firmware)={} tegra-RC={} ::", P2, okay, is_tegra_rc);
    if !is_tegra_rc {
        serial_println!(
            "{}   controller 0 is not a Tegra234 DesignWare RC (generic ecam / foreign) — link/device recon SKIPPED (graceful; QEMU virt) ::",
            P2
        );
        return;
    }
    if !okay {
        serial_println!(
            "{}   controller 0 left DISABLED by firmware — no link/device recon (bringing it up is a power/enable write => NET-3) ::",
            P2
        );
        return;
    }

    let dbi = region_by_name(&props, b"dbi");
    let cfg = region_by_name(&props, b"config");
    let ecam = region_by_name(&props, b"ecam");
    let appl = region_by_name(&props, b"appl");
    if let Some((b, s)) = appl {
        serial_println!("{}   region appl   = {:#x} (+{:#x}) ::", P2, b, s);
    }
    if let Some((b, s)) = dbi {
        serial_println!("{}   region dbi    = {:#x} (+{:#x}) ::", P2, b, s);
    }
    if let Some((b, s)) = cfg {
        serial_println!("{}   region config = {:#x} (+{:#x}) ::", P2, b, s);
    }
    if let Some((b, s)) = ecam {
        serial_println!("{}   region ecam   = {:#x} (+{:#x}) ::", P2, b, s);
    }

    // The MMIO mapping + reads are real Tegra hardware touches — tegra build only. On the virt witness
    // build we never reach here (the generic ecam is not a tegra-RC), so the block is compiled out.
    #[cfg(feature = "tegra")]
    net2_link_and_device(appl, dbi, cfg, ecam);
    #[cfg(not(feature = "tegra"))]
    {
        let _ = (appl, dbi, cfg, ecam);
        serial_println!(
            "{}   (non-tegra build: MMIO link/device recon not compiled — DTB scope only) ::",
            P2
        );
    }

    #[cfg(not(all(feature = "pcie3", feature = "tegra")))] // pcie2-only (or non-tegra): the page-table descriptors really ARE the only writes, so this literal stays byte-for-byte what `orin-net2-bench.md` §wire promises.
    serial_println!("{} ORIN-NET-2 controller-0 recon DONE (read-only; page-table mappings the only writes) ::", P2);
    #[cfg(all(feature = "pcie3", feature = "tegra"))] // ⚠ CENSUS2LIE (orin 11): the literal above ALSO printed here and was FALSE from 893fe5c7 on — boot7h/7i/7j carry it on the wire, so the archive keeps a read-only claim over a boot that enabled an LTSSM and sized BARs. Corrected forward only; captures are never edited. ⚠ FOLDED IN PLACE, never added lines (panic `Location` records embed line numbers). Reversal: arch_arm64.md §ORIN-NET-3.
    serial_println!("{} ORIN-NET-2 controller-0 preamble DONE — NOT read-only on this pcie3 image: past the page-table mappings this pass ARMS controller-0 fabric writes (the appl LTSSM enable, which can and does take the link from not-training to training; BAR-dword all-ones probes, each original restored the next statement, high half refused at slot 5). Which of them THIS boot issued is the `>>> FABRIC WRITE` lines above — read those, never this one. Bounding THIS PASS keeps, and nothing past it: controller 0 only, and outside those two classes no command-reg/decode-enable, LNKCTL, PERST, PHY, clock, power-domain or PSCI write, and no driver bind — on a `net4` image the driver's own decode-enable comes LATER, below this line ::", P2);
}

/// The metal half of NET-2 (tegra build only): map/reach controller-0's apertures via the kernel
/// page-table path (M1), read link state from the RP's DBI config space (M2), and — if the link is up —
/// walk the downstream device one level below (M2b). Every read is poison-rejecting.
///
/// WRITES — the same correction the file header carries, applied here because this doc comment stated
/// the identical false absolute. This USED to read "the ONLY writes are the Device-nGnRE page-table
/// descriptors `map_mmio_window` installs. No fabric/config/BAR write, no link retrain, no BAR
/// sizing." That is true of a `pcie2`-only build and FALSE with `pcie3` armed, which has been the case
/// since 893fe5c7 (2026-07-17) — this very function's `pcie3` block calls `net3_link_bringup` (the
/// APPL_CTRL LTSSM enable) and `net3_enumerate_and_size` (the BAR-sizing ritual). Under `pcie2` alone
/// the page-table descriptors ARE the only writes; under `pcie3` they are the least of them.
///
/// FIXED by CENSUS2LIE, 2026-09-01, and this paragraph is kept as the record of its own falsification
/// rather than silently replaced (`sdhc4c.rs`'s precedent). It read "KNOWN DEFECT, NOT FIXED HERE" and
/// described `census2` claiming "read-only" after this function enabled a link and sized BARs. The DONE
/// line is now `#[cfg]`-SPLIT — a flat rewrite would have made the `pcie2`-alone case newly false.
#[cfg(all(feature = "pcie2", feature = "tegra"))]
fn net2_link_and_device(
    appl: Option<(u64, u64)>,
    dbi: Option<(u64, u64)>,
    cfg: Option<(u64, u64)>,
    ecam: Option<(u64, u64)>,
) {
    use super::mmu_tegra::{map_mmio_window, MmioMap};
    // `appl` is only consumed by the NET-3 (`pcie3`) link-bring-up block below; a `pcie2`-only build
    // leaves it unused (and its behaviour byte-for-byte NET-2).
    #[cfg(not(feature = "pcie3"))]
    let _ = appl;

    // ── M1: reach the config/ECAM apertures via the EXISTING kernel page-table path ──
    let report_map = |name: &str, region: Option<(u64, u64)>| -> Option<u64> {
        let (base, size) = region?;
        match map_mmio_window(base, size as usize) {
            MmioMap::AlreadyMapped => {
                serial_println!(
                    "{}   map {} {:#x} (+{:#x}): ALREADY MAPPED (GiB-0/1 device window) — readable ::",
                    P2, name, base, size
                );
                Some(base)
            }
            MmioMap::Mapped => {
                serial_println!(
                    "{}   map {} {:#x} (+{:#x}): MAPPED Device-nGnRE (new page-table block) — readable ::",
                    P2, name, base, size
                );
                Some(base)
            }
            MmioMap::BeyondPsCeiling => {
                serial_println!(
                    "{}   map {} {:#x} (+{:#x}): BEYOND the 36-bit PS ceiling (GiB {} >= 64) — reaching it needs a TCR_EL2.PS widen to 40-bit, beyond a page-table write (STOP tripwire) => NET-3 must widen the tegra regime first ::",
                    P2, name, base, size, base >> 30
                );
                None
            }
        }
    };
    // NET-3 (`pcie3`) maps the appl aperture for the M2 LTSSM enable (GiB-0, already mapped); NET-2
    // never touched appl, so gate the extra map line under pcie3 to keep the pcie2-only dump identical.
    #[cfg(feature = "pcie3")]
    let appl_base = report_map("appl", appl);
    let dbi_base = report_map("dbi", dbi);
    let cfg_base = report_map("config", cfg);
    // NET-2 refused the ECAM (BeyondPsCeiling); NET-3's M1 PS widen makes it REACHABLE, and M3 walks
    // the downstream device through it (no iATU CFG-region fabric write needed — the ECAM is a direct
    // hardware config window). Under pcie2 the ECAM is still refused, so `ecam_base` stays `None`.
    let ecam_base = report_map("ecam", ecam);
    #[cfg(not(feature = "pcie3"))]
    let _ = ecam_base;

    // ── M2: link state from the RP's DBI config space (READ-ONLY, always valid regardless of link) ──
    let Some(dbi_base) = dbi_base else {
        serial_println!("{}   no reachable dbi aperture — link state UNREAD (NET-3) ::", P2);
        return;
    };
    // BDF(0,0,0) at the RP = DBI base. Poison-reject the identity word first (PI-V3D-1 rule).
    let vd = unsafe { core::ptr::read_volatile(dbi_base as *const u32) };
    serial_println!("{}   RP dbi[0x00] = {:#010x} ::", P2, vd);
    let Some((vendor, device)) = live_vendor_device(vd) else {
        serial_println!(
            "{}   RP ABSENT DECODE (poison/unclaimed) — controller powered down post-UEFI? link state UNREAD; STOP-record (RAS-safe: no further touch) ::",
            P2
        );
        return;
    };
    serial_println!("{}   RP LIVE: vendor={:#06x} device={:#06x} ::", P2, vendor, device);
    let cr = unsafe { core::ptr::read_volatile((dbi_base + 0x08) as *const u32) };
    if !is_poison(cr) {
        serial_println!(
            "{}   RP class={:#04x} subclass={:#04x} progif={:#04x} rev={:#04x} ::",
            P2,
            (cr >> 24) & 0xff,
            (cr >> 16) & 0xff,
            (cr >> 8) & 0xff,
            cr & 0xff
        );
    }
    let ht = unsafe { core::ptr::read_volatile((dbi_base + 0x0c) as *const u32) };
    if !is_poison(ht) {
        serial_println!(
            "{}   RP header-type={:#04x} (bit7=multifn; low7: 1=bridge) ::",
            P2,
            (ht >> 16) & 0xff
        );
    }

    // Walk the RP capability list to the PCIe capability (id 0x10) and read Link Status.
    let statusw = unsafe { core::ptr::read_volatile((dbi_base + 0x04) as *const u32) };
    let has_caps = (statusw >> 16) & (1 << 4) != 0; // Status.CapabilitiesList = bit 4 of the 16-bit status
    let mut ptr = if has_caps {
        (unsafe { core::ptr::read_volatile((dbi_base + 0x34) as *const u32) } & 0xff) as u64
    } else {
        0
    };
    let mut pcie_cap: u64 = 0;
    let mut hops = 0;
    while has_caps && ptr >= 0x40 && ptr < 0x100 && hops < 48 {
        let h = unsafe { core::ptr::read_volatile((dbi_base + ptr) as *const u32) };
        if is_poison(h) {
            break;
        }
        let id = h & 0xff;
        let next = (h >> 8) & 0xff;
        if id == 0x10 {
            pcie_cap = ptr;
            break;
        }
        if next == 0 || next as u64 == ptr {
            break;
        }
        ptr = next as u64;
        hops += 1;
    }

    let mut link_up = false;
    if pcie_cap != 0 {
        let linkcap = unsafe { core::ptr::read_volatile((dbi_base + pcie_cap + 0x0c) as *const u32) };
        let linkctlsta = unsafe { core::ptr::read_volatile((dbi_base + pcie_cap + 0x10) as *const u32) };
        if !is_poison(linkctlsta) {
            let lsta = linkctlsta >> 16; // Link Status is the high 16 bits of the Link Control/Status dword
            let cur_speed = lsta & 0xf;
            let cur_width = (lsta >> 4) & 0x3f;
            let dllla = (lsta >> 13) & 1; // Data Link Layer Link Active
            let max_speed = linkcap & 0xf;
            let max_width = (linkcap >> 4) & 0x3f;
            link_up = dllla == 1;
            serial_println!(
                "{}   PCIe cap @ {:#x}: LinkCap max(gen{},x{}) LinkStatus cur(gen{},x{}) DLL-active={} => LINK {} ::",
                P2, pcie_cap, max_speed, max_width, cur_speed, cur_width, dllla,
                if link_up { "UP" } else { "DOWN" }
            );
        } else {
            serial_println!("{}   PCIe cap Link Status read poison — link state INDETERMINATE ::", P2);
        }
    } else {
        serial_println!("{}   no PCIe capability in RP config space — link state UNREAD ::", P2);
    }

    // ── M2 (NET-3, pcie3): bring the link up on controller 0, then enumerate through the ECAM ──
    // The lane's FIRST DELIBERATE FABRIC WRITES live below (appl LTSSM enable + BAR sizing); every one
    // is logged loudly before issue. This whole branch is `pcie3`-gated and always returns, so the
    // NET-2 `pcie2` M2b (below, gated `not(pcie3)`) is a pcie2-only path — no dead/unreachable code.
    #[cfg(feature = "pcie3")]
    {
        if !link_up {
            link_up = net3_link_bringup(appl_base, dbi_base, pcie_cap);
        }
        if !link_up {
            serial_println!(
                "{}   controller-0 link STILL DOWN after the appl LTSSM-enable sequence => honest hardware result (the metal note's \"hardware question\" branch); no device below the RP. Further bring-up (PERST deassert / PHY retrain) is beyond the M2 enable sequence and this arc's three write classes — recorded, not improvised. ::",
                P3
            );
            return;
        }
        // Link up (as-left-by-firmware or after the enable): enumerate the downstream device through
        // the now-mapped ECAM (the direct hardware config window M1 unlocked — no iATU CFG-region
        // fabric write needed) and run the BAR-sizing ritual on it. `cfg_base` is the iATU-routed
        // fallback if the ECAM is somehow unreachable.
        net3_enumerate_and_size(ecam_base, cfg_base);
        return;
    }

    // ── M2b (NET-2 pcie2 path): one level below — only if the link is up ──
    #[cfg(not(feature = "pcie3"))]
    {
    if !link_up {
        serial_println!(
            "{}   link DOWN as-left-by-firmware => NO device enumerable below the root port. NET-3 scope: bring up / retrain the link (appl + PHY / LTSSM), then enumerate. ::",
            P2
        );
        return;
    }
    // Link up: the downstream device (bus1:dev0:fn0) is reachable through the `config` window (the
    // controller's iATU routes it) IF firmware left the CFG ATU region set. Poison-reject the read.
    let Some(cfg_base) = cfg_base else {
        serial_println!(
            "{}   link UP but no reachable config window — downstream walk needs the ECAM (NET-3) ::",
            P2
        );
        return;
    };
    let dv = unsafe { core::ptr::read_volatile(cfg_base as *const u32) };
    serial_println!("{}   bus1:dev0:fn0 config[0x00] = {:#010x} ::", P2, dv);
    let Some((dvendor, ddevice)) = live_vendor_device(dv) else {
        serial_println!(
            "{}   downstream ABSENT DECODE — no device answering (or the iATU CFG region is unset; programming it is a fabric write => NET-3) ::",
            P2
        );
        return;
    };
    serial_println!(
        "{}   DEVICE FOUND below RP: vendor={:#06x} device={:#06x} ::",
        P2, dvendor, ddevice
    );
    let dcr = unsafe { core::ptr::read_volatile((cfg_base + 0x08) as *const u32) };
    if !is_poison(dcr) {
        serial_println!(
            "{}   device class={:#04x} subclass={:#04x} progif={:#04x} rev={:#04x} ::",
            P2,
            (dcr >> 24) & 0xff,
            (dcr >> 16) & 0xff,
            (dcr >> 8) & 0xff,
            dcr & 0xff
        );
    }
    let dht = unsafe { core::ptr::read_volatile((cfg_base + 0x0c) as *const u32) };
    if !is_poison(dht) {
        serial_println!("{}   device header-type={:#04x} ::", P2, (dht >> 16) & 0xff);
    }
    // BAR0..5 raw (READ only; sizes UNKNOWN — the BAR-sizing write ritual is NET-3 territory).
    let mut bar = 0x10u64;
    let mut i = 0;
    while i < 6 {
        let v = unsafe { core::ptr::read_volatile((cfg_base + bar) as *const u32) };
        serial_println!(
            "{}   device BAR{} [{:#04x}] = {:#010x} (size UNKNOWN — no BAR sizing write){} ::",
            P2, i, bar, v,
            if is_poison(v) { " (poison/unimpl)" } else { "" }
        );
        bar += 4;
        i += 1;
    }
    serial_println!(
        "{}   downstream device identified read-only; BAR sizing + driver bind = NET-3 ::",
        P2
    );
    } // end #[cfg(not(feature = "pcie3"))] M2b block
}

// ══════════════════════════════════════════════════════════════════════════════════════════════════
// ORIN-NET-3 metal write-path (the `pcie3` + `tegra` build only) — M2 link bring-up + M3 BAR sizing.
// ══════════════════════════════════════════════════════════════════════════════════════════════════
//
// These functions perform the arc's ONLY deliberate fabric writes, in exactly the three brief-scoped
// classes (the TCR PS widen is the third, done in `mmu_tegra`/`boot_tegra` at MMU-enable). Every write
// is announced on serial BEFORE it is issued. Controller 0 ONLY; no other controller is touched, no
// bus-master/MEM decode is enabled, no MSI/DMA is set up. Any RAS/SError raised by a write lands in the
// `mmu_tegra` Part-C / healed `exceptions.rs` vectors (a recorded syndrome + spin), which IS the
// STOP-record for an unexpected fabric fault.
//
// Register map documentation of record: Linux `drivers/pci/controller/dwc/pcie-tegra194.c` (the
// Tegra194/234 DesignWare controller driver) for the `appl` block, and the PCI Local Bus spec for the
// standard config-space BAR-sizing ritual.

/// APPL block register offsets (Linux pcie-tegra194.c). `appl` base = controller-0 reg region "appl"
/// (`0x140a_0000`, inside the already-mapped GiB-0 Device window).
#[cfg(all(feature = "pcie3", feature = "tegra"))]
const APPL_CTRL: u64 = 0x4;
/// APPL_CTRL.LTSSM_EN — application-level enable of the DWC LTSSM (link-training state machine).
#[cfg(all(feature = "pcie3", feature = "tegra"))]
const APPL_CTRL_LTSSM_EN: u32 = 1 << 7;
/// APPL_LINK_STATUS.RDLH_LINK_UP — the appl-side link-up mirror (bit 0 of APPL_LINK_STATUS @ 0xCC).
#[cfg(all(feature = "pcie3", feature = "tegra"))]
const APPL_LINK_STATUS: u64 = 0xCC;
#[cfg(all(feature = "pcie3", feature = "tegra"))]
const APPL_LINK_STATUS_RDLH_LINK_UP: u32 = 1 << 0;
/// APPL_DEBUG (@ 0xD0) — LTSSM state in bits [8:3]; L0 (fully up) = 0x11.
#[cfg(all(feature = "pcie3", feature = "tegra"))]
const APPL_DEBUG: u64 = 0xD0;

/// Read DLL-active (Data-Link-Layer-Link-Active) from the RP's DBI PCIe-capability Link Status — the
/// same read path NET-2 used (dbi_base + pcie_cap + 0x10, high 16 bits, bit 13). `pcie_cap == 0` ⇒ no
/// PCIe cap located ⇒ report not-up (the caller also consults the appl-side mirror).
#[cfg(all(feature = "pcie3", feature = "tegra"))]
fn dbi_dll_active(dbi_base: u64, pcie_cap: u64) -> bool {
    if pcie_cap == 0 {
        return false;
    }
    let linkctlsta = unsafe { core::ptr::read_volatile((dbi_base + pcie_cap + 0x10) as *const u32) };
    if is_poison(linkctlsta) {
        return false;
    }
    ((linkctlsta >> 16) >> 13) & 1 == 1
}

/// ORIN-NET-3 M2: bring controller-0's link up with the `appl` LTSSM-enable sequence, then poll
/// DLL-active with a FINITE backstop. Returns the resolved link-up state (records it either way — a
/// still-down link after a correct enable is an honest hardware result, per the brief). The ONLY write
/// is a single read-modify-write of APPL_CTRL to set LTSSM_EN, announced before issue. No PERST/PHY
/// reprogramming (beyond the enable), no writes to any other controller.
#[cfg(all(feature = "pcie3", feature = "tegra"))]
fn net3_link_bringup(appl_base: Option<u64>, dbi_base: u64, pcie_cap: u64) -> bool {
    let Some(appl) = appl_base else {
        serial_println!(
            "{}   M2 link bring-up SKIPPED — appl aperture unreachable (no map) — cannot enable LTSSM ::",
            P3
        );
        return false;
    };
    serial_println!(
        "{}   M2 link bring-up on controller 0 (appl @ {:#x}) — Linux pcie-tegra194 LTSSM-enable sequence ::",
        P3, appl
    );
    // Read current APPL_CTRL, then announce and issue the LTSSM_EN set. This is FABRIC WRITE #1.
    let ctrl = unsafe { core::ptr::read_volatile((appl + APPL_CTRL) as *const u32) };
    let already = ctrl & APPL_CTRL_LTSSM_EN != 0;
    serial_println!(
        "{}   APPL_CTRL[{:#x}] = {:#010x} (LTSSM_EN currently {}) ::",
        P3, APPL_CTRL, ctrl, if already { "SET" } else { "clear" }
    );
    let newctrl = ctrl | APPL_CTRL_LTSSM_EN;
    serial_println!(
        "{}   >>> FABRIC WRITE (M2): APPL_CTRL[{:#x}] {:#010x} -> {:#010x} (set LTSSM_EN bit7) — issuing now ::",
        P3, APPL_CTRL, ctrl, newctrl
    );
    unsafe { core::ptr::write_volatile((appl + APPL_CTRL) as *mut u32, newctrl) };
    // Publish the write and give the fabric an ordering barrier before we start polling.
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
    serial_println!("{}   LTSSM_EN issued; polling DLL-active (finite backstop) ::", P3);

    // Finite backstop: bounded spins, each reading DLL-active (DBI) and the appl-side RDLH mirror.
    // ~2M spins is a generous ceiling (LTSSM to L0 is sub-ms); we break the instant either says up.
    const MAX_SPINS: u32 = 2_000_000;
    let mut up = false;
    let mut spins = 0u32;
    while spins < MAX_SPINS {
        let dll = dbi_dll_active(dbi_base, pcie_cap);
        let rdlh = {
            let ls = unsafe { core::ptr::read_volatile((appl + APPL_LINK_STATUS) as *const u32) };
            !is_poison(ls) && (ls & APPL_LINK_STATUS_RDLH_LINK_UP != 0)
        };
        if dll || rdlh {
            up = true;
            break;
        }
        core::hint::spin_loop();
        spins += 1;
    }
    // Record the LTSSM state from APPL_DEBUG either way (0x11 = L0 = fully up).
    let dbg = unsafe { core::ptr::read_volatile((appl + APPL_DEBUG) as *const u32) };
    let ltssm = if is_poison(dbg) { 0xff } else { (dbg >> 3) & 0x3f };
    let dll = dbi_dll_active(dbi_base, pcie_cap);
    serial_println!(
        "{}   M2 result after {} spins: DLL-active(DBI)={} APPL_DEBUG[{:#x}]={:#010x} LTSSM-state={:#04x}{} => LINK {} ::",
        P3, spins, dll, APPL_DEBUG, dbg, ltssm,
        if ltssm == 0x11 { " (L0)" } else { "" },
        if up { "UP" } else { "DOWN" }
    );
    up
}

/// ORIN-NET-3 M3: enumerate the downstream device and run the BAR-sizing ritual on it. Prefers the
/// now-mapped ECAM (the direct hardware config window M1 unlocked — bus1:dev0:fn0 = ecam_base +
/// (1<<20)); falls back to the iATU-routed `config` window if the ECAM is unreachable. Identity read
/// is poison-rejected (PI-V3D-1). BAR sizing is the standard all-ones/readback ritual, restoring each
/// original IMMEDIATELY, announced per write; 64-bit BARs size across both halves. No decode-enable,
/// no driver bind — recon stops at "device identified, BARs sized."
#[cfg(all(feature = "pcie3", feature = "tegra"))]
fn net3_enumerate_and_size(ecam_base: Option<u64>, cfg_base: Option<u64>) {
    // Resolve the downstream device's config base: ECAM bus1:dev0:fn0, else the iATU config window.
    let (dev, via) = match ecam_base {
        Some(e) => (e + (1u64 << 20), "ECAM bus1:dev0:fn0"),
        None => match cfg_base {
            Some(c) => (c, "iATU config window"),
            None => {
                serial_println!(
                    "{}   M3 enumerate SKIPPED — neither ECAM nor config window reachable ::",
                    P3
                );
                return;
            }
        },
    };
    serial_println!("{}   M3 enumerate downstream device via {} @ {:#x} ::", P3, via, dev);
    let dv = unsafe { core::ptr::read_volatile(dev as *const u32) };
    serial_println!("{}   device config[0x00] = {:#010x} ::", P3, dv);
    let Some((vendor, device)) = live_vendor_device(dv) else {
        serial_println!(
            "{}   downstream ABSENT DECODE (poison/unclaimed) — link up but no device answering (RP secondary-bus numbering unset by firmware?) — recorded, no further touch ::",
            P3
        );
        return;
    };
    serial_println!("{}   DEVICE FOUND: vendor={:#06x} device={:#06x} ::", P3, vendor, device);
    let cr = unsafe { core::ptr::read_volatile((dev + 0x08) as *const u32) };
    if !is_poison(cr) {
        serial_println!(
            "{}   class={:#04x} subclass={:#04x} progif={:#04x} rev={:#04x} ::",
            P3, (cr >> 24) & 0xff, (cr >> 16) & 0xff, (cr >> 8) & 0xff, cr & 0xff
        );
    }
    let ht = unsafe { core::ptr::read_volatile((dev + 0x0c) as *const u32) };
    if !is_poison(ht) {
        serial_println!("{}   header-type={:#04x} ::", P3, (ht >> 16) & 0xff);
    }

    // ── BAR sizing ritual (FABRIC WRITES: all-ones probe + immediate restore, per BAR) ──
    serial_println!("{}   M3 BAR sizing (all-ones/readback, restore-immediate; per-BAR fabric writes) ::", P3);
    let read = |off: u64| -> u32 { unsafe { core::ptr::read_volatile((dev + off) as *const u32) } };
    let write = |off: u64, v: u32| unsafe { core::ptr::write_volatile((dev + off) as *mut u32, v) };
    let mut i = 0u64;
    while i < 6 {
        let off = 0x10 + i * 4;
        let orig = read(off);
        serial_println!(
            "{}   >>> FABRIC WRITE (M3): BAR{}[{:#x}] all-ones probe (orig={:#010x}) — write 0xffffffff, read size, RESTORE ::",
            P3, i, off, orig
        );
        write(off, 0xffff_ffff);
        let readback = read(off);
        write(off, orig); // restore IMMEDIATELY
        serial_println!("{}   BAR{} restored to {:#010x} (readback was {:#010x}) ::", P3, i, orig, readback);
        if readback == 0 {
            serial_println!("{}   BAR{} unimplemented (readback 0) ::", P3, i);
            i += 1;
            continue;
        }
        if orig & 1 == 1 {
            // I/O-space BAR (uncommon on this class); size mask ignores the low 2 bits.
            let mask = readback & !0x3;
            let size = (!mask).wrapping_add(1) & 0xffff;
            serial_println!("{}   BAR{} = I/O space, size={:#x} ::", P3, i, size);
            i += 1;
            continue;
        }
        let mem_type = (orig >> 1) & 0x3; // 0=32-bit, 2=64-bit
        let prefetch = (orig >> 3) & 1;
        let mask = readback & !0xf;
        if mem_type == 0x2 && i == 5 {
            // WRITE-SCOPE GUARD: a 64-bit BAR's high half is the NEXT BAR dword, but there is no BAR
            // beyond slot 5 — probing it would drive an all-ones write to config offset 0x28 (the
            // Cardbus CIS pointer), OUTSIDE the "enumerated device's BARs" write class. A 64-bit type
            // in slot 5 is a malformed/misread BAR: record it and STOP touching (never write off 0x28).
            serial_println!(
                "{}   BAR5 reports 64-bit type but has no high-half BAR (malformed/misread) — sizing SKIPPED, no write past the BAR array ::",
                P3
            );
            i += 1;
        } else if mem_type == 0x2 {
            // 64-bit memory BAR: the high half is the next BAR — probe + restore it too.
            let hoff = off + 4;
            let horig = read(hoff);
            serial_println!(
                "{}   >>> FABRIC WRITE (M3): BAR{}[{:#x}] (64-bit high half, orig={:#010x}) — write 0xffffffff, read size, RESTORE ::",
                P3, i + 1, hoff, horig
            );
            write(hoff, 0xffff_ffff);
            let hread = read(hoff);
            write(hoff, horig); // restore IMMEDIATELY
            serial_println!("{}   BAR{} (high) restored to {:#010x} (readback {:#010x}) ::", P3, i + 1, horig, hread);
            let full_mask = ((hread as u64) << 32) | (mask as u64);
            let size = (!full_mask).wrapping_add(1);
            serial_println!(
                "{}   BAR{}/{} = 64-bit mem (prefetch={}), size={:#x} ::",
                P3, i, i + 1, prefetch, size
            );
            i += 2;
        } else {
            let size = ((!(mask as u64)) & 0xffff_ffff).wrapping_add(1);
            serial_println!(
                "{}   BAR{} = 32-bit mem (prefetch={}), size={:#x} ::",
                P3, i, prefetch, size
            );
            i += 1;
        }
    }
    serial_println!(
        "{}   M3 DONE — device identified + BARs sized (originals restored); no decode-enable / no driver bind (NIC driver arc is next). ::",
        P3
    );
}
