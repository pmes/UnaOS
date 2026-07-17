// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// ORIN-NET-1 — read-only PCIe root-complex + NIC recon (`pcieprobe` gated).
//
// Orin has no network path yet. The Jetson Orin Nano devkit's NIC sits behind the Tegra234 PCIe
// root complex, so networking begins by knowing exactly what the firmware (NVIDIA UEFI / L4T
// 39.2.0) left us at ExitBootServices. This module is the SMP-2-style *census-before-touch* that
// scopes the real bring-up chain (PCIe RC -> NIC -> smoltcp, already in-tree). It writes NOTHING to
// fabric or config space, enables no clock or power domain, retrains no link, and changes no power
// state — the wall (JETSON-XCARVE) taught this track to census before it touches.
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
// ## Read-only invariant (the arc's review lens)
//
// Every access in this module is a `read_volatile` or a DTB byte read. There is no `write_volatile`,
// no `msr`/config write, no `SET_*` mailbox/MRQ, no `CPU_ON`, no link retrain, no BAR/command-reg
// write. `GET_STATE`-class reads are the only firmware queries this arc would ever add (none are
// needed here — the DTB `status` is the enable oracle). The compiled module is gated
// `#[cfg(feature = "pcieprobe")]`; with the knob OFF (default) the module and its call sites vanish
// and the image is byte-identical to baseline.

use super::fdt_tegra::Fdt;

/// Stable serial prefix so the operator (and `mbench`) can grep the whole census as one block.
const P: &str = ":: PCIE:";

/// The top of the `mmu_tegra` GiB-0 Device-nGnRE window (0x0..0x4000_0000). A config/appl aperture
/// below this is already mapped and readable at EL2; at or above it is UNMAPPED (the high Tegra234
/// PCIe config apertures live here) — reading it would need a page-table write, which this arc will
/// not do (STOP tripwire). See `mmu_tegra::init` (`L1[0]` = the low-1-GiB Device window).
const GIB0_DEVICE_TOP: u64 = 0x4000_0000;

/// Poison patterns that mean ABSENT DECODE, never "present" (PI-V3D-1 false-PASS lesson).
///  - `0xffffffff`: the PCIe master-abort / unclaimed-config return (no responder at that BDF).
///  - `0xdeadbeef`: firmware register/DRAM fill (the exact V3D false-PASS value).
#[inline]
fn is_poison(v: u32) -> bool {
    v == 0xffff_ffff || v == 0xdead_beef
}

/// A config-space `vendor:device` word is LIVE only if it is neither a poison fill nor the two
/// all-decode-boundary values (`0x0000_0000` = powered-off/no responder, `0xffff_????` vendor =
/// unclaimed). Returns `Some((vendor, device))` on a live decode, `None` on absent decode.
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
    // header then the cells inline using a fold over a fixed writer is overkill — print per-line.
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
struct EnabledDecode {
    okay: bool,
    /// The base of the `reg` region named "config" (or the DW `dbi` fallback), if resolvable.
    config_base: Option<u64>,
    config_kind: &'static str,
}

/// Resolve whether the firmware left this controller ENABLED and where its config aperture sits.
/// READ-ONLY: `status` string test + a `reg`/`reg-names` index lookup. Never touches MMIO.
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
