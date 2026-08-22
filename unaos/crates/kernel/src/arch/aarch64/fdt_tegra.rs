// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// JB1a: read the BPMP IPC geometry off the firmware's OWN device tree (the Jetson Orin `tegra`
// path). The Orin keyboard/mouse/video arcs are all gated behind talking to the BPMP (JX1: gated
// Tegra blocks are EL3-fatal to touch, so the XUSB/nvdisplay partitions must be ungated via BPMP
// MRQs first). The BPMP IVC channel needs three facts the Linux upstream dtsi can only *suggest*
// for this exact L4T firmware: the shmem TX/RX locations, the HSP doorbell block, and which
// doorbell rings the BPMP. The authoritative source is the DTB NVIDIA's UEFI publishes (the
// bootloader captures it into `BootInfo::dtb_addr/dtb_size` from the EFI config table), so JB1a
// walks that DTB READ-ONLY and prints the geometry — a zero-MMIO-risk attended boot.
//
// Deliberately hand-rolled: the `fdt` crate (0.1.5) is known to panic on this board's DTB (the
// UNAOS_BOOTDIAG fdt panic, JM2 era). This walker is a bounded big-endian token scan — no
// allocation (runs pre-heap at EL2), every offset checked against the blob bounds, iteration
// capped — so a malformed blob degrades to a printed "not found", never a fault or a hang.
//
// FDT format (devicetree spec v0.4, flattened blob): a header of big-endian u32s (magic
// 0xd00dfeed @0, totalsize @4, off_dt_struct @8, off_dt_strings @12), then a structure block of
// 4-byte tokens: BEGIN_NODE(1) + NUL-padded name, END_NODE(2), PROP(3) + {len, nameoff} + value
// padded to 4, NOP(4), END(9). Property names live in the strings block at `nameoff`.

const FDT_MAGIC: u32 = 0xd00d_feed;
const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_NOP: u32 = 4;
const FDT_END: u32 = 9;
/// Token-iteration cap: far above any real DTB (the Orin blob is ~300 KiB), far below forever.
const MAX_TOKENS: usize = 400_000;
/// Depth cap for the node-name stack (real trees are < 10 deep).
const MAX_DEPTH: usize = 16;
/// Longest node-name path we keep (bytes, '/'-joined).
const MAX_PATH: usize = 160;
/// Most raw u32 words we capture from one property (the XUSB `clocks` list is 9 [phandle, id]
/// pairs = 18 words — keep headroom above that).
const MAX_WORDS: usize = 20;

#[inline]
fn be32(d: &[u8], off: usize) -> Option<u32> {
    let b = d.get(off..off + 4)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// NUL-terminated string starting at `off`, capped at 64 bytes. Returns (bytes, padded-end).
fn cstr(d: &[u8], off: usize) -> Option<(&[u8], usize)> {
    let mut end = off;
    let cap = (off + 64).min(d.len());
    while end < cap && d[end] != 0 {
        end += 1;
    }
    if end >= cap && d.get(end).copied() != Some(0) && end != off {
        // Unterminated within cap — take what we have (diagnostic tool, not a validator).
    }
    Some((&d[off..end], (end + 1 + 3) & !3))
}

/// A captured property value: up to MAX_WORDS raw big-endian u32s + the true length.
#[derive(Clone, Copy)]
pub struct PropWords {
    pub words: [u32; MAX_WORDS],
    pub n: usize,
    pub total_len: usize,
    pub found: bool,
}

impl PropWords {
    const fn empty() -> Self {
        PropWords { words: [0; MAX_WORDS], n: 0, total_len: 0, found: false }
    }
    fn capture(d: &[u8], off: usize, len: usize) -> Self {
        let mut p = PropWords { words: [0; MAX_WORDS], n: 0, total_len: len, found: true };
        let mut o = off;
        while p.n < MAX_WORDS && o + 4 <= off + len {
            if let Some(w) = be32(d, o) {
                p.words[p.n] = w;
                p.n += 1;
            } else {
                break;
            }
            o += 4;
        }
        p
    }
}

/// One walk event: a property, delivered with the '/'-joined path of its owning node.
pub struct PropEvent<'a> {
    pub path: &'a [u8],
    pub depth: usize,
    pub name: &'a [u8],
    pub val_off: usize,
    pub val_len: usize,
}

pub struct Fdt<'a> {
    d: &'a [u8],
    off_struct: usize,
    off_strings: usize,
}

impl<'a> Fdt<'a> {
    /// Validate the header against the blob bounds. `None` = not a usable DTB (caller prints why).
    pub fn new(d: &'a [u8]) -> Option<Fdt<'a>> {
        if be32(d, 0)? != FDT_MAGIC {
            return None;
        }
        let total = be32(d, 4)? as usize;
        let off_struct = be32(d, 8)? as usize;
        let off_strings = be32(d, 12)? as usize;
        if total > d.len() || off_struct >= total || off_strings >= total {
            return None;
        }
        Some(Fdt { d: &d[..total], off_struct, off_strings })
    }

    /// Walk every property in the structure block, calling `f` with the owning node's path.
    /// Bounded by MAX_TOKENS; malformed structure ends the walk early (silently — diagnostic).
    pub fn for_each_prop(&self, mut f: impl FnMut(&PropEvent<'_>)) {
        let d = self.d;
        let mut off = self.off_struct;
        // The '/'-joined path buffer + per-depth end offsets, so END_NODE truncates in O(1).
        let mut path = [0u8; MAX_PATH];
        let mut path_len = 0usize;
        let mut ends = [0usize; MAX_DEPTH];
        let mut depth = 0usize;
        for _ in 0..MAX_TOKENS {
            let Some(tok) = be32(d, off) else { return };
            off += 4;
            match tok {
                FDT_BEGIN_NODE => {
                    let Some((name, next)) = cstr(d, off) else { return };
                    off = next;
                    if depth < MAX_DEPTH {
                        ends[depth] = path_len;
                        // Append "/name" (truncate silently if the buffer fills — paths this
                        // deep are beyond anything we query). The ROOT node's name is EMPTY —
                        // skip the append entirely so a top-level node is "/bpmp", not "//bpmp"
                        // (the JB1a first-boot lesson: the double slash made every path matcher
                        // miss).
                        if !name.is_empty() {
                            if path_len < MAX_PATH {
                                path[path_len] = b'/';
                                path_len += 1;
                            }
                            for &b in name {
                                if path_len < MAX_PATH {
                                    path[path_len] = b;
                                    path_len += 1;
                                }
                            }
                        }
                    }
                    depth += 1;
                }
                FDT_END_NODE => {
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                    if depth < MAX_DEPTH {
                        path_len = ends[depth];
                    }
                }
                FDT_PROP => {
                    let Some(len) = be32(d, off) else { return };
                    let Some(nameoff) = be32(d, off + 4) else { return };
                    let val_off = off + 8;
                    let len = len as usize;
                    if val_off + len > d.len() {
                        return;
                    }
                    let Some((pname, _)) = cstr(d, self.off_strings + nameoff as usize) else {
                        return;
                    };
                    f(&PropEvent {
                        path: &path[..path_len],
                        depth,
                        name: pname,
                        val_off,
                        val_len: len,
                    });
                    off = val_off + ((len + 3) & !3);
                }
                FDT_NOP => {}
                FDT_END => return,
                _ => return, // unknown token: stop rather than misparse
            }
        }
    }

    /// Raw words of property `prop` on the node at exactly `path` (first match).
    pub fn prop_at(&self, path: &[u8], prop: &[u8]) -> PropWords {
        let mut out = PropWords::empty();
        self.for_each_prop(|e| {
            if !out.found && e.path == path && e.name == prop {
                out = PropWords::capture(self.d, e.val_off, e.val_len);
            }
        });
        out
    }

    /// Find the path of the node carrying `phandle`/`linux,phandle` == `ph`. Returns the length
    /// of the path copied into `buf` (0 = not found).
    pub fn path_of_phandle(&self, ph: u32, buf: &mut [u8; MAX_PATH]) -> usize {
        let mut n = 0usize;
        self.for_each_prop(|e| {
            if n == 0
                && (e.name == b"phandle" || e.name == b"linux,phandle")
                && e.val_len == 4
                && be32(self.d, e.val_off) == Some(ph)
            {
                let l = e.path.len().min(MAX_PATH);
                buf[..l].copy_from_slice(&e.path[..l]);
                n = l;
            }
        });
        n
    }
}

/// Helper: parent path = everything before the last '/'.
fn parent(path: &[u8]) -> &[u8] {
    match path.iter().rposition(|&b| b == b'/') {
        Some(0) | None => b"",
        Some(i) => &path[..i],
    }
}

struct W([u32; MAX_WORDS], usize);
impl core::fmt::Display for W {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "[")?;
        for i in 0..self.1 {
            if i > 0 {
                write!(f, " ")?;
            }
            write!(f, "{:#x}", self.0[i])?;
        }
        write!(f, "]")
    }
}
fn w(p: &PropWords) -> W {
    W(p.words, p.n)
}

/// The resolved BPMP IPC geometry (JB1b): absolute PAs computed from the DTB the firmware
/// published — shmem child `reg` = [offset, size] under a parent whose `reg` = [hi lo hi lo].
pub struct BpmpGeom {
    pub shmem_tx: u64,
    pub shmem_rx: u64,
    pub hsp_base: u64,
    /// mboxes[2] — the HSP doorbell MASTER id (19 = BPMP on Tegra234); the doorbell INDEX is a
    /// separate constant (3) from the Linux tegra-hsp db_map.
    pub db_master: u32,
}

/// Resolve the JB1b geometry (the JB1a dump is the human-readable twin). `None` = any piece missing
/// or shaped unexpectedly — the caller prints its generic SKIP, so EVERY rung here names itself on
/// serial first (the SDID lesson: a ladder that gives up through a bare `?` leaves a capture unable
/// to say which rung died). The success path stays silent — `jb1b_ping` prints the geometry it got.
pub fn bpmp_geometry(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) -> Option<BpmpGeom> {
    if dtb_addr == 0 || dtb_size == 0 {
        return geom_stop("no DTB handed off", dtb_addr, dtb_size as u64);
    }
    let g_lo = dtb_addr >> 30;
    let g_hi = (dtb_addr + dtb_size as u64 - 1) >> 30;
    let mapped = |g: u64| g == 0 || (g < 64 && (ram_gib_mask >> g) & 1 != 0);
    if !mapped(g_lo) || !mapped(g_hi) {
        return geom_stop("DTB GiB unmapped (lo..hi vs RAM-GiB-mask)", g_lo, ram_gib_mask);
    }
    let blob = unsafe { core::slice::from_raw_parts(dtb_addr as *const u8, dtb_size) };
    let Some(fdt) = Fdt::new(blob) else {
        return geom_stop(
            "bad DTB header (magic/bounds) at Fdt::new; first two words",
            be32(blob, 0).unwrap_or(0) as u64,
            be32(blob, 4).unwrap_or(0) as u64,
        );
    };

    let mut bpmp_path = [0u8; MAX_PATH];
    let mut bpmp_len = 0usize;
    fdt.for_each_prop(|e| {
        if bpmp_len == 0 && e.depth == 2 && e.path.len() >= 5 && &e.path[1..5] == b"bpmp" {
            let l = e.path.len().min(MAX_PATH);
            bpmp_path[..l].copy_from_slice(&e.path[..l]);
            bpmp_len = l;
        }
    });
    if bpmp_len == 0 {
        return geom_stop("no top-level /bpmp* node in the tree", 0, 0);
    }
    let bp = &bpmp_path[..bpmp_len];
    let mboxes = fdt.prop_at(bp, b"mboxes");
    let shmem = fdt.prop_at(bp, b"shmem");
    if mboxes.n < 3 || shmem.n < 2 {
        return geom_stop(
            "/bpmp mboxes/shmem too short (need >=3 / >=2 cells); got mboxes.n / shmem.n",
            mboxes.n as u64,
            shmem.n as u64,
        );
    }

    // A shmem phandle target: child reg [offset, size] + parent reg [hi lo hi lo] -> absolute PA.
    let resolve_shmem = |ph: u32, which: &str| -> Option<u64> {
        let mut buf = [0u8; MAX_PATH];
        let n = fdt.path_of_phandle(ph, &mut buf);
        if n == 0 {
            serial_println!(
                ":: tegra: JB1b-GEOM STOP — shmem {} phandle {:#x} resolves to no node; BPMP channel unresolvable ::",
                which, ph
            );
            return None;
        }
        let node = &buf[..n];
        let reg = fdt.prop_at(node, b"reg");
        let preg = fdt.prop_at(parent(node), b"reg");
        if reg.n < 1 || preg.n < 2 {
            serial_println!(
                ":: tegra: JB1b-GEOM STOP — shmem {} node {} reg.n={} / parent reg.n={} (need >=1 / >=2); cannot form the absolute PA ::",
                which,
                core::str::from_utf8(node).unwrap_or("?"),
                reg.n,
                preg.n,
            );
            return None;
        }
        let parent_base = ((preg.words[0] as u64) << 32) | preg.words[1] as u64;
        Some(parent_base + reg.words[0] as u64)
    };
    let shmem_tx = resolve_shmem(shmem.words[0], "TX")?;
    let shmem_rx = resolve_shmem(shmem.words[1], "RX")?;

    // The HSP node behind mboxes[0]: reg [hi lo hi lo] -> base.
    let mut hbuf = [0u8; MAX_PATH];
    let hn = fdt.path_of_phandle(mboxes.words[0], &mut hbuf);
    if hn == 0 {
        return geom_stop(
            "HSP phandle (mboxes[0]) resolves to no node; doorbell base unknown — phandle",
            mboxes.words[0] as u64,
            0,
        );
    }
    let hreg = fdt.prop_at(&hbuf[..hn], b"reg");
    if hreg.n < 2 {
        return geom_stop("HSP node reg.n too short (need >=2 cells for the base); reg.n", hreg.n as u64, 0);
    }
    let hsp_base = ((hreg.words[0] as u64) << 32) | hreg.words[1] as u64;
    Some(BpmpGeom { shmem_tx, shmem_rx, hsp_base, db_master: mboxes.words[2] })
}

/// One named JB1b-GEOM abandon witness + the `None` the rung returns, so a rung stays a single
/// `return geom_stop(..)` line and no call-site line numbers shift (the tail-helper convention).
#[inline(never)]
fn geom_stop(why: &str, a: u64, b: u64) -> Option<BpmpGeom> {
    serial_println!(":: tegra: JB1b-GEOM STOP — {} = {:#x} / {:#x}; BPMP geometry unresolved ::", why, a, b);
    None
}

/// JB1c: the XUSB host node's BPMP resource IDs, read off the firmware DTB (never a header
/// guess). `clocks`/`resets`/`power-domains` are [phandle, id] pairs — we keep the ids.
pub struct XusbIds {
    // 9 slots: the tegra234.dtsi usb@3610000 `clocks` list has NINE entries — an 8-slot cap
    // silently dropped the 9th (TEGRA234_CLK_PLLE, the UPHY/SS 100 MHz PLL; JB5 finding).
    pub clocks: [u32; 9],
    pub n_clocks: usize,
    pub resets: [u32; 4],
    pub n_resets: usize,
    pub pds: [u32; 4],
    pub n_pds: usize,
}

/// Find the node whose name contains "usb@3610000" and read its BPMP resource id lists.
///
/// Every abandon rung names itself on serial (`JB1c-IDS STOP`): the caller's line is the generic
/// "no usb@3610000 ids in DTB; SKIP", which cannot say whether the DTB was absent, unmapped,
/// malformed, node-less, or merely id-less — and without ids the whole XUSB ungate is skipped.
pub fn xusb_ids(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) -> Option<XusbIds> { #[cfg(feature = "jd1dc")] if let Some(ids) = node_ids(dtb_addr, dtb_size, ram_gib_mask, b"usb@3610000") { return Some(ids); } // JD1-DC: on an ARMED build these ids come from the GENERALISED reader at this file's tail — the same walk with the node name lifted into a parameter, which is what lets the nvdisplay probe's MRQ_PG guard read `display@13800000`'s power-domains with this code instead of a second copy of it. APPENDED to this line, never a new one, so every `core::panic::Location` below keeps its line number and the disarmed image is untouched. FALL-THROUGH, not replacement: `None` drops into the original body below, so a bug in the generic reader can cost an armed flight one extra STOP line and never its keyboard.
    if dtb_addr == 0 || dtb_size == 0 {
        return ids_stop("no DTB handed off", dtb_addr, dtb_size as u64);
    }
    let g_lo = dtb_addr >> 30;
    let g_hi = (dtb_addr + dtb_size as u64 - 1) >> 30;
    let mapped = |g: u64| g == 0 || (g < 64 && (ram_gib_mask >> g) & 1 != 0);
    if !mapped(g_lo) || !mapped(g_hi) {
        return ids_stop("DTB GiB unmapped (lo vs RAM-GiB-mask)", g_lo, ram_gib_mask);
    }
    let blob = unsafe { core::slice::from_raw_parts(dtb_addr as *const u8, dtb_size) };
    let Some(fdt) = Fdt::new(blob) else {
        return ids_stop(
            "bad DTB header (magic/bounds) at Fdt::new; first two words",
            be32(blob, 0).unwrap_or(0) as u64,
            be32(blob, 4).unwrap_or(0) as u64,
        );
    };
    let mut path = [0u8; MAX_PATH];
    let mut plen = 0usize;
    fdt.for_each_prop(|e| {
        if plen == 0 && e.path.windows(11).any(|w| w == b"usb@3610000") {
            let l = e.path.len().min(MAX_PATH);
            path[..l].copy_from_slice(&e.path[..l]);
            plen = l;
        }
    });
    if plen == 0 {
        return ids_stop("no node whose path contains usb@3610000", 0, 0);
    }
    let node = &path[..plen];
    // [phandle, id] pairs -> collect the odd-index words.
    let pick = |p: &PropWords, out: &mut [u32], n: &mut usize| {
        let mut i = 1;
        while i < p.n && *n < out.len() {
            out[*n] = p.words[i];
            *n += 1;
            i += 2;
        }
    };
    let mut ids = XusbIds {
        clocks: [0; 9],
        n_clocks: 0,
        resets: [0; 4],
        n_resets: 0,
        pds: [0; 4],
        n_pds: 0,
    };
    let clocks = fdt.prop_at(node, b"clocks");
    let resets = fdt.prop_at(node, b"resets");
    let pds = fdt.prop_at(node, b"power-domains");
    pick(&clocks, &mut ids.clocks, &mut ids.n_clocks);
    pick(&resets, &mut ids.resets, &mut ids.n_resets);
    pick(&pds, &mut ids.pds, &mut ids.n_pds);
    if ids.n_clocks == 0 && ids.n_resets == 0 {
        return ids_stop(
            "usb@3610000 found but carries NO clocks and NO resets (raw prop cell counts clocks.n / resets.n)",
            clocks.n as u64,
            resets.n as u64,
        );
    }
    Some(ids)
}

/// One named JB1c-IDS abandon witness + the `None` the rung returns (tail-helper convention: the
/// rung stays one line, so no call-site `Location` shifts).
#[inline(never)]
fn ids_stop(why: &str, a: u64, b: u64) -> Option<XusbIds> {
    serial_println!(":: tegra: JB1c-IDS STOP — {} = {:#x} / {:#x}; XUSB resource ids unresolved ::", why, a, b);
    None
}

/// JB2b: does the firmware DTB mark the XUSB host node `dma-coherent`? A DT boolean is presence
/// (`val_len == 0`), so `prop_at(..).found` is the test. `Some(true)` = the controller's DMA snoops
/// the CPU caches (Normal-WB rings need no cache maintenance — the Linux tegra234.dtsi stance);
/// `Some(false)` = the prop is absent from this firmware's tree (verify-don't-assume: the attach
/// still proceeds, and a stale-ring enumeration stall becomes the diagnosis); `None` = the node or
/// the DTB itself was unresolvable. Same node match as `xusb_ids`.
pub fn xusb_dma_coherent(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) -> Option<bool> {
    // `None` here is NOT the same as `Some(false)` — it means the question was never asked, and the
    // attach then runs its cache-maintenance choice on an unresolved answer. The consumer lives in
    // `xusb_tegra`, so the naming has to happen here.
    if dtb_addr == 0 || dtb_size == 0 {
        return xusb_prop_stop("dma-coherent: no DTB handed off (addr / size)", dtb_addr, dtb_size as u64);
    }
    let g_lo = dtb_addr >> 30;
    let g_hi = (dtb_addr + dtb_size as u64 - 1) >> 30;
    let mapped = |g: u64| g == 0 || (g < 64 && (ram_gib_mask >> g) & 1 != 0);
    if !mapped(g_lo) || !mapped(g_hi) {
        return xusb_prop_stop("dma-coherent: DTB GiB unmapped (GiB lo / RAM-GiB-mask)", g_lo, ram_gib_mask);
    }
    let blob = unsafe { core::slice::from_raw_parts(dtb_addr as *const u8, dtb_size) };
    let Some(fdt) = Fdt::new(blob) else {
        return xusb_prop_stop("dma-coherent: bad DTB header at Fdt::new (addr / size)", dtb_addr, dtb_size as u64);
    };
    let mut path = [0u8; MAX_PATH];
    let mut plen = 0usize;
    fdt.for_each_prop(|e| {
        if plen == 0 && e.path.windows(11).any(|w| w == b"usb@3610000") {
            let l = e.path.len().min(MAX_PATH);
            path[..l].copy_from_slice(&e.path[..l]);
            plen = l;
        }
    });
    if plen == 0 {
        return xusb_prop_stop("dma-coherent: no node whose path contains usb@3610000", 0, 0);
    }
    Some(fdt.prop_at(&path[..plen], b"dma-coherent").found)
}

/// One named witness for the two `usb@3610000`/`padctl@3520000` property lookups whose `None` is
/// consumed inside `xusb_tegra` (a lane this arc does not touch) — so the abandon still lands on the
/// wire. `Option<T>`-generic so both the `bool` and `u64` resolvers can use it.
#[inline(never)]
fn xusb_prop_stop<T>(why: &str, a: u64, b: u64) -> Option<T> {
    serial_println!(":: tegra: JB-DTBPROP STOP — {} = {:#x} / {:#x}; property unresolved ::", why, a, b);
    None
}

/// JB9: the XUSB padctl AO aperture base — reg region 1 of the `padctl@3520000` node. The JB8
/// edk2-nvidia source read pinned the IFR-autoboot registers there (`IFRDMA_CFG0` @AO+0x1bc,
/// `IFRDMA_CFG1` @AO+0x1c0, `IFRDMA_STREAMID` @AO+0x1c4 — "padctl DT reg region 1"); resolving
/// the base from the LIVE firmware DTB (verify-don't-assume) instead of hardcoding it keeps an
/// unverified-aperture touch (the JX1 EL3-fatal class) impossible. `reg` on /bus@0 children is
/// (addr:2, size:2) cells, so region 1's base is words[4..6]. None = node/region absent.
pub fn xusb_padctl_ao(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) -> Option<u64> {
    if dtb_addr == 0 || dtb_size == 0 {
        return xusb_prop_stop("padctl AO: no DTB handed off (addr / size)", dtb_addr, dtb_size as u64);
    }
    let g_lo = dtb_addr >> 30;
    let g_hi = (dtb_addr + dtb_size as u64 - 1) >> 30;
    let mapped = |g: u64| g == 0 || (g < 64 && (ram_gib_mask >> g) & 1 != 0);
    if !mapped(g_lo) || !mapped(g_hi) {
        return xusb_prop_stop("padctl AO: DTB GiB unmapped (GiB lo / RAM-GiB-mask)", g_lo, ram_gib_mask);
    }
    let blob = unsafe { core::slice::from_raw_parts(dtb_addr as *const u8, dtb_size) };
    let Some(fdt) = Fdt::new(blob) else {
        return xusb_prop_stop("padctl AO: bad DTB header at Fdt::new (addr / size)", dtb_addr, dtb_size as u64);
    };
    let mut path = [0u8; MAX_PATH];
    let mut plen = 0usize;
    fdt.for_each_prop(|e| {
        if plen == 0 && e.path.windows(14).any(|w| w == b"padctl@3520000") {
            let l = e.path.len().min(MAX_PATH);
            path[..l].copy_from_slice(&e.path[..l]);
            plen = l;
        }
    });
    if plen == 0 {
        return xusb_prop_stop("padctl AO: no node whose path contains padctl@3520000", 0, 0);
    }
    let reg = fdt.prop_at(&path[..plen], b"reg");
    if reg.n < 8 {
        return xusb_prop_stop("padctl AO: reg.n < 8 — no region-1 (AO) aperture in the node; reg.n", reg.n as u64, 0);
    }
    let base = ((reg.words[4] as u64) << 32) | reg.words[5] as u64;
    if base == 0 {
        return xusb_prop_stop("padctl AO: region-1 base decoded as 0 (reg words[4]/[5])", reg.words[4] as u64, reg.words[5] as u64);
    }
    Some(base)
}

/// JB1a: print the BPMP IPC geometry from the firmware DTB. Read-only; pre-heap; EL2. Every
/// outcome is one serial line — including every way of failing to find things.
pub fn jb1a_dump(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) {
    serial_println!(":: tegra: JB1a — DTB @{:#x} size={:#x} ::", dtb_addr, dtb_size);
    if dtb_addr == 0 || dtb_size == 0 {
        serial_println!(":: tegra: JB1a — no DTB handed off; STOP (geometry unavailable) ::");
        return;
    }
    // The blob must lie in mapped memory: a RAM GiB from the mask, or the GiB-0 device window.
    let g_lo = dtb_addr >> 30;
    let g_hi = (dtb_addr + dtb_size as u64 - 1) >> 30;
    let mapped =
        |g: u64| g == 0 || (g < 64 && (ram_gib_mask >> g) & 1 != 0);
    if !mapped(g_lo) || !mapped(g_hi) {
        serial_println!(
            ":: tegra: JB1a — DTB GiB {}..{} unmapped (mask {:#x}); STOP (map it next arc rev) ::",
            g_lo,
            g_hi,
            ram_gib_mask
        );
        return;
    }
    let blob = unsafe { core::slice::from_raw_parts(dtb_addr as *const u8, dtb_size) };
    let Some(fdt) = Fdt::new(blob) else {
        serial_println!(
            ":: tegra: JB1a — bad DTB header (magic/bounds); first words {:#x} {:#x} ::",
            be32(blob, 0).unwrap_or(0),
            be32(blob, 4).unwrap_or(0)
        );
        return;
    };

    // 1. The bpmp node: any top-level node whose name starts with "bpmp". Capture its path +
    //    the three properties that define the channel.
    let mut bpmp_path = [0u8; MAX_PATH];
    let mut bpmp_len = 0usize;
    fdt.for_each_prop(|e| {
        // A top-level node's properties arrive at depth 2 (root push = 1, node push = 2).
        if bpmp_len == 0 && e.depth == 2 {
            // e.path is "/<name>"; check the name component.
            if e.path.len() >= 5 && &e.path[1..5] == b"bpmp" {
                let l = e.path.len().min(MAX_PATH);
                bpmp_path[..l].copy_from_slice(&e.path[..l]);
                bpmp_len = l;
            }
        }
    });
    if bpmp_len == 0 {
        serial_println!(":: tegra: JB1a — no /bpmp node found ::");
    } else {
        let bp = &bpmp_path[..bpmp_len];
        let mboxes = fdt.prop_at(bp, b"mboxes");
        let shmem = fdt.prop_at(bp, b"shmem");
        let memreg = fdt.prop_at(bp, b"memory-region");
        serial_println!(
            ":: tegra: JB1a — {} mboxes{} shmem{} memory-region{} ::",
            core::str::from_utf8(bp).unwrap_or("/bpmp"),
            w(&mboxes),
            w(&shmem),
            w(&memreg),
        );
        // 2. Resolve each phandle target: its node path + raw `reg`, plus the parent's `reg` +
        //    `ranges` (sram children are offsets that the parent node translates).
        let mut targets = [0u32; 4];
        let mut nt = 0;
        for i in 0..shmem.n.min(2) {
            targets[nt] = shmem.words[i];
            nt += 1;
        }
        for i in 0..memreg.n.min(2) {
            targets[nt] = memreg.words[i];
            nt += 1;
        }
        for &ph in &targets[..nt] {
            let mut pbuf = [0u8; MAX_PATH];
            let plen = fdt.path_of_phandle(ph, &mut pbuf);
            if plen == 0 {
                serial_println!(":: tegra: JB1a — phandle {:#x}: NOT FOUND ::", ph);
                continue;
            }
            let node = &pbuf[..plen];
            let reg = fdt.prop_at(node, b"reg");
            let par = parent(node);
            let preg = fdt.prop_at(par, b"reg");
            let pranges = fdt.prop_at(par, b"ranges");
            serial_println!(
                ":: tegra: JB1a — ph {:#x} = {} reg{} | parent {} reg{} ranges{} ::",
                ph,
                core::str::from_utf8(node).unwrap_or("?"),
                w(&reg),
                core::str::from_utf8(par).unwrap_or("/"),
                w(&preg),
                w(&pranges),
            );
        }
    }

    // 3. Reserved-memory: print every child whose name mentions bpmp/shmem (the DRAM-carveout
    //    variant of the channel), raw reg words.
    let mut printed = 0u32;
    fdt.for_each_prop(|e| {
        // /reserved-memory/<child> properties arrive at depth 3 (root=1, reserved-memory=2).
        if e.name == b"reg" && e.depth == 3 && printed < 6 {
            let p = e.path;
            let under_resv = p.len() > 16 && &p[..16] == b"/reserved-memory";
            if under_resv {
                let name = &p[17.min(p.len())..];
                let interesting = name.windows(4).any(|q| q == b"bpmp")
                    || name.windows(5).any(|q| q == b"shmem");
                if interesting {
                    let words = PropWords::capture(blob, e.val_off, e.val_len);
                    serial_println!(
                        ":: tegra: JB1a — resv {} reg{} ::",
                        core::str::from_utf8(p).unwrap_or("?"),
                        w(&words),
                    );
                    printed += 1;
                }
            }
        }
    });
    if printed == 0 {
        serial_println!(":: tegra: JB1a — no bpmp/shmem reserved-memory children ::");
    }
}

/// ORIN-VUG-RAS — enumerate the firmware's DRAM carveouts from the DTB `/reserved-memory` children
/// as (base, size) byte ranges. On Tegra234 several carveouts (TZ, BPMP, DCE, ...) are protected by
/// the SNOC memory firewall AND are reported as ordinary Conventional (Usable) DRAM in the UEFI map,
/// so the kernel heap must exclude them explicitly: a cached store into one succeeds into the
/// D-cache and then faults on writeback with an SNOC RAS Uncorrectable "Carveout" abort that powers
/// the cores off (the vug lockup — a heap store into a carveout page, evicted a few frames later).
/// `/reserved-memory` on Tegra uses 2 address + 2 size cells (root #address-cells/#size-cells = 2).
/// Only children carrying a concrete `reg` are returned; dynamically-placed carveouts (declared with
/// `size` + `alloc-ranges` and no static base) can't be excluded from here — the caller notes that
/// the heap-guard witness line prints the final range so a bench sitting can cross-check the RAS PA.
/// Read-only RAM walk (the dtb is Normal-WB-mapped by the time `memory::init` runs). Returns the
/// number of ranges written into `out`.
pub fn reserved_carveouts(dtb_addr: u64, dtb_size: usize, out: &mut [(u64, u64)]) -> usize {
    if dtb_addr == 0 || dtb_size == 0 || dtb_size > 4 * 1024 * 1024 || out.is_empty() {
        return 0;
    }
    let blob = unsafe { core::slice::from_raw_parts(dtb_addr as *const u8, dtb_size) };
    let Some(fdt) = Fdt::new(blob) else { return 0 };
    let mut n = 0usize;
    fdt.for_each_prop(|e| {
        // /reserved-memory/<child> `reg` arrives at depth 3 (root=1, reserved-memory=2, child=3).
        if n >= out.len() || e.name != b"reg" || e.depth != 3 {
            return;
        }
        let p = e.path;
        if !(p.len() > 16 && &p[..16] == b"/reserved-memory") {
            return;
        }
        let words = PropWords::capture(blob, e.val_off, e.val_len);
        // Each reg entry is [base_hi, base_lo, size_hi, size_lo]; a node may carry several. Skip
        // zero-size entries.
        let mut i = 0usize;
        while i + 4 <= words.n && n < out.len() {
            let base = ((words.words[i] as u64) << 32) | words.words[i + 1] as u64;
            let size = ((words.words[i + 2] as u64) << 32) | words.words[i + 3] as u64;
            if size != 0 {
                out[n] = (base, size);
                n += 1;
            }
            i += 4;
        }
    });
    n
}

/// ORIN-DMA-WINDOW — derive the PCIe controller's INBOUND-DMA window(s) from the DTB `dma-ranges`.
///
/// The RAS-2 class (heap seated below the inbound-DMA window ⇒ the fabric translated the NIC's ring
/// writebacks to ~0x0..0x200 ⇒ SNOC/IOB RAS Uncorrectable, cores off) was patched with a HEURISTIC
/// (`select_heap_region` prefers the highest clean window). The real boundary is DERIVABLE, not
/// folklore: a DesignWare/Tegra234 PCIe root complex declares its inbound bus→CPU windows in
/// `dma-ranges`. Each row on THIS hardware (child #address-cells=3, parent #address-cells=2,
/// #size-cells=2 ⇒ 3+2+2 = 7 cells = 28 bytes — the same stride the node's `ranges` uses, walked
/// identically in `rtl8168_tegra::resolve_atu_and_window`) maps an incoming PCIe address to a PARENT
/// (CPU/DRAM) address for `size` bytes; the parent side `[cpu_base, cpu_base+size)` is the window a
/// bus-master write must land in to reach DRAM — below it is the RAS-2 fabric-error class. Writes each
/// `(cpu_base, size)` into `out` and returns the count. READ-ONLY (the dtb is Normal-WB-mapped by the
/// time `memory::init`/the NIC bring-up run).
///
/// Gated to the first firmware-`okay` Tegra DesignWare RC node (`tegra234/194-pcie` / `snps,dw-pcie`)
/// — the SAME gate `resolve_ecam_base`/`resolve_atu_and_window` apply — so a QEMU-virt DTB (a generic
/// `pcie@` that is not Tegra-compatible) yields 0 windows and the caller degrades to the highest-clean
/// heuristic with a witness. A missing node, an absent `dma-ranges`, or a foreign DTB all return 0.
/// (`nvidia,dma-*` props observed on Tegra234 are booleans — e.g. `dma-coherent` — not address
/// windows; `dma-ranges` is the sole inbound-window source, so this parses exactly that.)
pub fn pcie_dma_windows(dtb_addr: u64, dtb_size: usize, out: &mut [(u64, u64)]) -> usize {
    if dtb_addr == 0 || dtb_size == 0 || dtb_size > 4 * 1024 * 1024 || out.is_empty() {
        return dmaw_stop("DTB absent or implausibly sized (addr / size)", dtb_addr, dtb_size as u64);
    }
    let blob = unsafe { core::slice::from_raw_parts(dtb_addr as *const u8, dtb_size) };
    let Some(fdt) = Fdt::new(blob) else {
        return dmaw_stop(
            "bad DTB header (magic/bounds) at Fdt::new; first two words",
            be32(blob, 0).unwrap_or(0) as u64,
            be32(blob, 4).unwrap_or(0) as u64,
        );
    };

    // First `pcie@` node's path (mirrors resolve_atu_and_window's walk).
    let mut path0 = [0u8; MAX_PATH];
    let mut plen0 = 0usize;
    let mut found = false;
    fdt.for_each_prop(|e| {
        if found {
            return;
        }
        let leaf = match e.path.iter().rposition(|&b| b == b'/') {
            Some(i) => &e.path[i + 1..],
            None => e.path,
        };
        if leaf.starts_with(b"pcie@") {
            let l = e.path.len().min(MAX_PATH);
            path0[..l].copy_from_slice(&e.path[..l]);
            plen0 = l;
            found = true;
        }
    });
    if !found {
        return dmaw_stop("no pcie@ node in the tree", 0, 0);
    }
    let path = &path0[..plen0];

    // compatible / status / dma-ranges of that node.
    let mut compatible: Option<&[u8]> = None;
    let mut status: Option<&[u8]> = None;
    let mut dma_ranges: Option<&[u8]> = None;
    fdt.for_each_prop(|e| {
        if e.path != path {
            return;
        }
        let val = &blob[e.val_off..e.val_off + e.val_len];
        match e.name {
            b"compatible" => compatible = Some(val),
            b"status" => status = Some(val),
            b"dma-ranges" => dma_ranges = Some(val),
            _ => {}
        }
    });

    // Tegra DesignWare RC + firmware-enabled? (same gate as resolve_atu_and_window / resolve_ecam_base.)
    let is_tegra_rc = compatible
        .map(|c| {
            let has = |n: &[u8]| c.windows(n.len()).any(|w| w == n);
            has(b"tegra234-pcie") || has(b"tegra194-pcie") || has(b"snps,dw-pcie")
        })
        .unwrap_or(false);
    if !is_tegra_rc {
        return dmaw_stop(
            "first pcie@ node is not a Tegra/DesignWare RC (compatible present?/len) — foreign or QEMU-virt DTB",
            compatible.is_some() as u64,
            compatible.map(|c| c.len() as u64).unwrap_or(0),
        );
    }
    let okay = match status {
        None => true,
        Some(s) => s.split(|&b| b == 0).any(|item| item == b"okay" || item == b"ok"),
    };
    if !okay {
        return dmaw_stop("Tegra RC node status is NOT okay (firmware-disabled controller); status len", status.map(|s| s.len() as u64).unwrap_or(0), 0);
    }

    let Some(dma_ranges) = dma_ranges else {
        return dmaw_stop("Tegra RC node is okay but carries NO dma-ranges property", 0, 0);
    };
    let cell = |b: &[u8], i: usize| -> u64 {
        u32::from_be_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]) as u64
    };
    let mut n = 0usize;
    let mut off = 0usize;
    while off + 28 <= dma_ranges.len() && n < out.len() {
        let row = &dma_ranges[off..off + 28];
        // child 3 cells (0=phys.hi/space, 1/2=PCI addr — inbound, identity on Tegra); parent 2 cells
        // (3/4 = CPU/DRAM base); size 2 cells (5/6). The parent side is the inbound-DMA window.
        let cpu_base = (cell(row, 3) << 32) | cell(row, 4);
        let size = (cell(row, 5) << 32) | cell(row, 6);
        if size != 0 {
            out[n] = (cpu_base, size);
            n += 1;
        }
        off += 28;
    }
    if n == 0 {
        return dmaw_stop("dma-ranges present but yielded 0 usable rows (byte length / 28-byte row stride)", dma_ranges.len() as u64, 28);
    }
    n
}

/// One named DMA-WINDOW abandon witness + the `0` the rung returns. `select_heap_region`'s degrade
/// line only says "NO PCIe dma-ranges in DTB"; that is true of FIVE different failures, and the
/// difference decides whether the RAS-2 heap boundary is derivable at all on this board.
#[inline(never)]
fn dmaw_stop(why: &str, a: u64, b: u64) -> usize {
    serial_println!(
        ":: tegra: DMA-WINDOW STOP — {} = {:#x} / {:#x}; inbound-DMA window NOT derivable (heap falls back to the RAS-2 heuristic) ::",
        why, a, b
    );
    0
}

/// JB3 boot-6: census of EVERY smmu/iommu node the firmware DTB knows — name, compatible,
/// first reg base. Boot-5's verdict (v2 pair open + fault-free, DMA still dead) means a second
/// killer sits downstream; Tegra234 carries SMMUv3 instances alongside the MMU-500 pairs, and
/// this walk finds their bases WITHOUT touching any unknown address (read-only RAM). Returns
/// the first base whose compatible contains "smmu-v3" (the dump candidate), if any.
pub fn smmu_census(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) -> Option<u64> {
    if dtb_addr == 0 || dtb_size == 0 {
        return None;
    }
    let g_lo = dtb_addr >> 30;
    let g_hi = (dtb_addr + dtb_size as u64 - 1) >> 30;
    let mapped = |g: u64| g == 0 || (g < 64 && (ram_gib_mask >> g) & 1 != 0);
    if !mapped(g_lo) || !mapped(g_hi) {
        return None;
    }
    let blob = unsafe { core::slice::from_raw_parts(dtb_addr as *const u8, dtb_size) };
    let fdt = Fdt::new(blob)?;
    // Collect unique matching node paths (bounded), then print each once with compatible+reg.
    const MAX_NODES: usize = 8;
    let mut paths = [[0u8; MAX_PATH]; MAX_NODES];
    let mut lens = [0usize; MAX_NODES];
    let mut n = 0usize;
    fdt.for_each_prop(|e| {
        let hit = e.path.windows(4).any(|w| w == b"smmu")
            || e.path.windows(6).any(|w| w == b"iommu@");
        if !hit || n >= MAX_NODES {
            return;
        }
        let l = e.path.len().min(MAX_PATH);
        let dup = (0..n).any(|i| lens[i] == l && &paths[i][..l] == &e.path[..l]);
        if !dup {
            paths[n][..l].copy_from_slice(&e.path[..l]);
            lens[n] = l;
            n += 1;
        }
    });
    let mut v3_base: Option<u64> = None;
    for i in 0..n {
        let node = &paths[i][..lens[i]];
        let compat = fdt.prop_at(node, b"compatible");
        let mut cbuf = [b' '; 44];
        let mut cl = 0usize;
        let mut is_v3 = false;
        'fill: for wi in 0..compat.n {
            for b in compat.words[wi].to_be_bytes() {
                if cl >= cbuf.len() {
                    break 'fill;
                }
                cbuf[cl] = if b == 0 { b'|' } else if b.is_ascii_graphic() { b } else { b'?' };
                cl += 1;
            }
        }
        // "smmu-v3" anywhere in the ASCII view
        if cl >= 7 {
            for w in cbuf[..cl].windows(7) {
                if w == b"smmu-v3" {
                    is_v3 = true;
                }
            }
        }
        let reg = fdt.prop_at(node, b"reg");
        let base = if reg.n >= 2 {
            ((reg.words[0] as u64) << 32) | reg.words[1] as u64
        } else {
            0
        };
        serial_println!(
            ":: tegra: JB3 — census {}: reg0={:#010x} compat='{}'{} ::",
            core::str::from_utf8(node).unwrap_or("?"),
            base,
            core::str::from_utf8(&cbuf[..cl]).unwrap_or("?"),
            if is_v3 { " *V3*" } else { "" }
        );
        if is_v3 && v3_base.is_none() && base != 0 {
            v3_base = Some(base);
        }
    }
    if n == 0 {
        serial_println!(":: tegra: JB3 — census: no smmu/iommu nodes found ::");
    }
    v3_base
}

/// JB3: the XUSB host node's `iommus` binding, resolved to the SMMU node it names — stream id
/// + the SMMU instance base(s) + a bounded ASCII view of its `compatible`, all off the LIVE
/// firmware DTB (verify-don't-assume: mainline tegra234.dtsi predicts
/// `<&smmu_niso1 TEGRA234_SID_XUSB_HOST=0x0e>` with the dual MMU-500 at 0x0800_0000 +
/// 0x0700_0000 — this reads what THIS board's firmware actually says).
pub struct XusbIommu {
    pub sid: u32,
    pub bases: [u64; 2],
    pub n_bases: usize,
}

/// Read-only RAM walk (no MMIO). Prints as it resolves — every failure mode is one serial
/// line — and returns `None` if the tree doesn't cooperate (the caller then probes the
/// predicted bases, saying so). Same node match + GiB guard as `xusb_ids`.
pub fn xusb_iommu(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) -> Option<XusbIommu> {
    if dtb_addr == 0 || dtb_size == 0 {
        return None;
    }
    let g_lo = dtb_addr >> 30;
    let g_hi = (dtb_addr + dtb_size as u64 - 1) >> 30;
    let mapped = |g: u64| g == 0 || (g < 64 && (ram_gib_mask >> g) & 1 != 0);
    if !mapped(g_lo) || !mapped(g_hi) {
        return None;
    }
    let blob = unsafe { core::slice::from_raw_parts(dtb_addr as *const u8, dtb_size) };
    let fdt = Fdt::new(blob)?;
    let mut path = [0u8; MAX_PATH];
    let mut plen = 0usize;
    fdt.for_each_prop(|e| {
        if plen == 0 && e.path.windows(11).any(|w| w == b"usb@3610000") {
            let l = e.path.len().min(MAX_PATH);
            path[..l].copy_from_slice(&e.path[..l]);
            plen = l;
        }
    });
    if plen == 0 {
        serial_println!(":: tegra: JB3 — no usb@3610000 node in DTB ::");
        return None;
    }
    let iommus = fdt.prop_at(&path[..plen], b"iommus");
    if !iommus.found || iommus.n < 2 {
        serial_println!(":: tegra: JB3 — usb@3610000 has no iommus prop (n={}) ::", iommus.n);
        return None;
    }
    let (ph, sid) = (iommus.words[0], iommus.words[1]);
    let mut spath = [0u8; MAX_PATH];
    let slen = fdt.path_of_phandle(ph, &mut spath);
    if slen == 0 {
        serial_println!(
            ":: tegra: JB3 — iommus phandle {:#x} unresolved (sid={:#x}) ::",
            ph,
            sid
        );
        return None;
    }
    let snode = &spath[..slen];
    // Bus-level #address-cells=2 / #size-cells=2: reg entries are [addr-hi addr-lo size-hi
    // size-lo]; the dual-MMU-500 node carries two entries (one per mirrored instance).
    let reg = fdt.prop_at(snode, b"reg");
    let mut out = XusbIommu {
        sid,
        bases: [0; 2],
        n_bases: 0,
    };
    let mut w = 0usize;
    while w + 3 < reg.n && out.n_bases < 2 {
        out.bases[out.n_bases] = ((reg.words[w] as u64) << 32) | reg.words[w + 1] as u64;
        out.n_bases += 1;
        w += 4;
    }
    // `compatible` is a NUL-joined string list riding the BE words — a bounded ASCII view is
    // enough to tell smmu-500 (v2) from smmu-v3 on the serial line.
    let compat = fdt.prop_at(snode, b"compatible");
    let mut cbuf = [b' '; 44];
    let mut cl = 0usize;
    'fill: for wi in 0..compat.n {
        for b in compat.words[wi].to_be_bytes() {
            if cl >= cbuf.len() {
                break 'fill;
            }
            cbuf[cl] = match b {
                0 => b'|',
                b if b.is_ascii_graphic() => b,
                _ => b'?',
            };
            cl += 1;
        }
    }
    serial_println!(
        ":: tegra: JB3 — DTB: {} sid={:#x} -> {} reg0={:#010x} reg1={:#010x} ({} inst) compat='{}' ::",
        core::str::from_utf8(&path[..plen]).unwrap_or("usb@3610000"),
        sid,
        core::str::from_utf8(snode).unwrap_or("?"),
        out.bases[0],
        out.bases[1],
        out.n_bases,
        core::str::from_utf8(&cbuf[..cl]).unwrap_or("?")
    );
    if out.n_bases == 0 {
        return None;
    }
    Some(out)
}

/// NET-4i: PCIe controller-0's SMMU binding, resolved off the LIVE firmware DTB — the SMMU
/// instance base(s) plus the stream id that controller-0's DMA emits. Parallel to [`xusb_iommu`],
/// but a PCIe root complex declares its stream via `iommu-map` (an RID→streamid map:
/// `<rid-base &smmu sid-base length>` 4-tuples) rather than the point-to-point `iommus` XUSB uses.
/// A downstream single-function device (bus1:dev0:fn0, the NET-4 RTL8168) has RID 0, so the FIRST
/// tuple's `sid-base` is the stream id it presents to the SMMU. Falls back to a plain `iommus`
/// binding if the node uses that form. Same first-`pcie@`-node selection as `resolve_ecam_base`
/// (controller-0 = `/bus@0/pcie@140a0000`). READ-ONLY RAM walk; `None` on any uncooperative tree.
pub struct PcieIommu {
    pub sid: u32,
    pub bases: [u64; 2],
    pub n_bases: usize,
}

pub fn pcie_iommu(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) -> Option<PcieIommu> {
    if dtb_addr == 0 || dtb_size == 0 {
        return None;
    }
    let g_lo = dtb_addr >> 30;
    let g_hi = (dtb_addr + dtb_size as u64 - 1) >> 30;
    let mapped = |g: u64| g == 0 || (g < 64 && (ram_gib_mask >> g) & 1 != 0);
    if !mapped(g_lo) || !mapped(g_hi) {
        return None;
    }
    let blob = unsafe { core::slice::from_raw_parts(dtb_addr as *const u8, dtb_size) };
    let fdt = Fdt::new(blob)?;

    // First `pcie@` node (controller-0), the same node `resolve_ecam_base` drives.
    let mut path = [0u8; MAX_PATH];
    let mut plen = 0usize;
    fdt.for_each_prop(|e| {
        if plen != 0 {
            return;
        }
        let leaf = match e.path.iter().rposition(|&b| b == b'/') {
            Some(i) => &e.path[i + 1..],
            None => e.path,
        };
        if leaf.starts_with(b"pcie@") {
            let l = e.path.len().min(MAX_PATH);
            path[..l].copy_from_slice(&e.path[..l]);
            plen = l;
        }
    });
    if plen == 0 {
        serial_println!(":: tegra: [net4i] no pcie@ node in DTB — cannot resolve PCIe SMMU stream ::");
        return None;
    }

    // `iommu-map` (RID→streamid) preferred; RID 0's tuple is [rid-base, phandle, sid-base, length].
    // Some node revisions carry a plain `iommus = <&smmu sid>`; accept that too.
    let map = fdt.prop_at(&path[..plen], b"iommu-map");
    let (ph, sid, form) = if map.found && map.n >= 3 {
        (map.words[1], map.words[2], "iommu-map")
    } else {
        let it = fdt.prop_at(&path[..plen], b"iommus");
        if it.found && it.n >= 2 {
            (it.words[0], it.words[1], "iommus")
        } else {
            serial_println!(
                ":: tegra: [net4i] {} has no iommu-map/iommus prop — PCIe SMMU stream unknown ::",
                core::str::from_utf8(&path[..plen]).unwrap_or("pcie@")
            );
            return None;
        }
    };

    let mut spath = [0u8; MAX_PATH];
    let slen = fdt.path_of_phandle(ph, &mut spath);
    if slen == 0 {
        serial_println!(
            ":: tegra: [net4i] SMMU phandle {:#x} unresolved (sid={:#x}, via {}) ::",
            ph, sid, form
        );
        return None;
    }
    let snode = &spath[..slen];
    // Dual MMU-500: #address-cells=2/#size-cells=2, so reg is [addr-hi addr-lo size-hi size-lo]
    // per mirrored instance (same shape xusb_iommu decodes).
    let reg = fdt.prop_at(snode, b"reg");
    let mut out = PcieIommu {
        sid,
        bases: [0; 2],
        n_bases: 0,
    };
    let mut w = 0usize;
    while w + 3 < reg.n && out.n_bases < 2 {
        out.bases[out.n_bases] = ((reg.words[w] as u64) << 32) | reg.words[w + 1] as u64;
        out.n_bases += 1;
        w += 4;
    }
    let compat = fdt.prop_at(snode, b"compatible");
    let mut cbuf = [b' '; 44];
    let mut cl = 0usize;
    'fill: for wi in 0..compat.n {
        for b in compat.words[wi].to_be_bytes() {
            if cl >= cbuf.len() {
                break 'fill;
            }
            cbuf[cl] = match b {
                0 => b'|',
                b if b.is_ascii_graphic() => b,
                _ => b'?',
            };
            cl += 1;
        }
    }
    serial_println!(
        ":: tegra: [net4i] DTB: {} sid={:#x} (via {}) -> {} reg0={:#010x} reg1={:#010x} ({} inst) compat='{}' ::",
        core::str::from_utf8(&path[..plen]).unwrap_or("pcie@"),
        sid,
        form,
        core::str::from_utf8(snode).unwrap_or("?"),
        out.bases[0],
        out.bases[1],
        out.n_bases,
        core::str::from_utf8(&cbuf[..cl]).unwrap_or("?")
    );
    if out.n_bases == 0 {
        return None;
    }
    Some(out)
}

/// JD1 (video): the firmware's live scanout framebuffer, taken from the DTB `simple-framebuffer`
/// handoff — the SIMPLEFB display-handoff the JetPack UEFI uses (edk2-nvidia
/// `DisplayDeviceTreeHelperLib`). The GOP is `BltOnly` precisely *because* the firmware hands the
/// framebuffer off through the device tree instead: MB2/TegraBL allocates a DRAM carveout
/// (`CARVEOUT_DISP_EARLY_BOOT_FB`) that the DCE scans out, and the UEFI display driver writes
/// `width/height/stride/format` onto a `compatible = "simple-framebuffer"` node (under /chosen) and
/// the **physical** base/size into that node's `memory-region` reserved-memory `reg`, with
/// `iommu-addresses` declaring the display-IOMMU *identity* map (so the reg PA equals the DC's
/// scanout IOVA). Reading this is a pure RAM walk — no display MMIO, no SMMU translation, no
/// double-buffer active/assembly hazard, no powergate touch — the purest "inherit, don't re-init".
pub struct SimpleFb {
    /// Physical scanout base (the EARLY_FB sub-carveout PA the firmware already page-aligned).
    pub base: u64,
    pub size: u64,
    pub width: u32,
    pub height: u32,
    /// Line stride in BYTES (the simplefb binding's `stride`, already in bytes — unlike the DC's
    /// `PLANAR_STORAGE`, which is bytes/64).
    pub stride: u32,
    /// The `format` string, e.g. `"x8r8g8b8"` (BGRX in memory) / `"x8b8g8r8"` (RGBX in memory).
    pub format: [u8; 16],
    pub format_len: usize,
    /// PA came from a `memory-region` reserved-memory node (true) vs a `reg` on the fb node (false).
    pub via_memregion: bool,
}

/// Longest simple-framebuffer node list we keep (one per active head; Orin Nano lights one).
const MAX_FB_NODES: usize = 4;

/// Resolve the DTB `simple-framebuffer` handoff silently (the `jd1_dump` twin is the human-readable
/// form). Returns the first non-`disabled` node that carries a resolvable physical base + geometry.
/// `None` = no such node (the firmware did not publish the SIMPLEFB handoff into the DTB we were
/// handed, or the DTB is unmapped/malformed) — the caller then prints and skips the blit.
pub fn nvdisplay_simplefb(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) -> Option<SimpleFb> {
    if dtb_addr == 0 || dtb_size == 0 {
        return sfb_stop("no DTB handed off", dtb_addr, dtb_size as u64);
    }
    let g_lo = dtb_addr >> 30;
    let g_hi = (dtb_addr + dtb_size as u64 - 1) >> 30;
    let mapped = |g: u64| g == 0 || (g < 64 && (ram_gib_mask >> g) & 1 != 0);
    if !mapped(g_lo) || !mapped(g_hi) {
        return sfb_stop("DTB GiB unmapped (lo vs RAM-GiB-mask)", g_lo, ram_gib_mask);
    }
    let blob = unsafe { core::slice::from_raw_parts(dtb_addr as *const u8, dtb_size) };
    let Some(fdt) = Fdt::new(blob) else {
        return sfb_stop(
            "bad DTB header (magic/bounds) at Fdt::new; first two words",
            be32(blob, 0).unwrap_or(0) as u64,
            be32(blob, 4).unwrap_or(0) as u64,
        );
    };

    // Pass 1: collect every node whose `compatible` names "simple-framebuffer" (the firmware writes
    // one per active head under /chosen). Bounded.
    let mut paths = [[0u8; MAX_PATH]; MAX_FB_NODES];
    let mut lens = [0usize; MAX_FB_NODES];
    let mut n = 0usize;
    fdt.for_each_prop(|e| {
        if n >= MAX_FB_NODES || e.name != b"compatible" {
            return;
        }
        let end = (e.val_off + e.val_len).min(blob.len());
        if blob[e.val_off..end].windows(18).any(|q| q == b"simple-framebuffer") {
            let l = e.path.len().min(MAX_PATH);
            let dup = (0..n).any(|i| lens[i] == l && &paths[i][..l] == &e.path[..l]);
            if !dup {
                paths[n][..l].copy_from_slice(&e.path[..l]);
                lens[n] = l;
                n += 1;
            }
        }
    });

    // Pass 2: take the first node that is not `status = "disabled"` and resolves to a sane base.
    for i in 0..n {
        let node = &paths[i][..lens[i]];
        // The firmware sets status="okay" only on the active head; skip any explicitly disabled one.
        let status = fdt.prop_at(node, b"status");
        if status.found {
            let mut sb = [0u8; 12];
            let mut sl = 0usize;
            'sfill: for wi in 0..status.n {
                for b in status.words[wi].to_be_bytes() {
                    if b == 0 || sl >= sb.len() {
                        break 'sfill;
                    }
                    sb[sl] = b;
                    sl += 1;
                }
            }
            if sl >= 8 && &sb[..8] == b"disabled" {
                sfb_reject(node, "status = \"disabled\" (firmware marks this head inactive)", 0, 0);
                continue;
            }
        }
        let width = fdt.prop_at(node, b"width");
        let height = fdt.prop_at(node, b"height");
        let stride = fdt.prop_at(node, b"stride");
        let format = fdt.prop_at(node, b"format");
        let memregion = fdt.prop_at(node, b"memory-region");
        let node_reg = fdt.prop_at(node, b"reg");

        // Physical base+size: prefer the `memory-region` reserved-memory node's `reg` (the
        // /reserved-memory bus is 2 address + 2 size cells → [base_hi base_lo size_hi size_lo]);
        // fall back to a `reg` directly on the fb node (same cell shape under /chosen).
        let (base, size, via_memregion) = if memregion.found && memregion.n >= 1 {
            let mut rbuf = [0u8; MAX_PATH];
            let rn = fdt.path_of_phandle(memregion.words[0], &mut rbuf);
            if rn == 0 {
                sfb_reject(node, "memory-region phandle resolves to no node", memregion.words[0] as u64, 0);
                continue;
            }
            let rr = fdt.prop_at(&rbuf[..rn], b"reg");
            if rr.n < 4 {
                sfb_reject(node, "memory-region node reg.n < 4 (need base_hi/lo + size_hi/lo); reg.n", rr.n as u64, 0);
                continue;
            }
            (
                ((rr.words[0] as u64) << 32) | rr.words[1] as u64,
                ((rr.words[2] as u64) << 32) | rr.words[3] as u64,
                true,
            )
        } else if node_reg.n >= 4 {
            (
                ((node_reg.words[0] as u64) << 32) | node_reg.words[1] as u64,
                ((node_reg.words[2] as u64) << 32) | node_reg.words[3] as u64,
                false,
            )
        } else {
            sfb_reject(node, "no memory-region phandle and node reg.n < 4; no physical base; reg.n", node_reg.n as u64, 0);
            continue;
        };
        // Require a DRAM base (>= 0x8000_0000) here too — so a bad-but-resolvable first node does not
        // mask a valid later one (the full geometry/size sanity lives in display_tegra::jd1_survey).
        if base < 0x8000_0000 || width.n == 0 || height.n == 0 {
            sfb_reject(node, "base below DRAM (<0x80000000) or width/height absent; base / (width.n<<8|height.n)", base, ((width.n as u64) << 8) | height.n as u64);
            continue;
        }
        let mut fmt = [0u8; 16];
        let mut fmt_len = 0usize;
        'ffill: for wi in 0..format.n {
            for b in format.words[wi].to_be_bytes() {
                if b == 0 || fmt_len >= 16 {
                    break 'ffill;
                }
                fmt[fmt_len] = b;
                fmt_len += 1;
            }
        }
        return Some(SimpleFb {
            base,
            size,
            width: width.words[0],
            height: height.words[0],
            stride: if stride.n >= 1 { stride.words[0] } else { 0 },
            format: fmt,
            format_len: fmt_len,
            via_memregion,
        });
    }
    sfb_stop(
        "no simple-framebuffer node survived resolution (candidate node count / all rejected above)",
        n as u64,
        0,
    )
}

/// One named JD1-SFB abandon witness + the `None` the rung returns. Without it the resolver's
/// failure is a bare `None` and the boot goes headless with no line saying WHY — the exact
/// ambiguity the `jd1_dump` (which lists the nodes it *found*) cannot resolve on its own.
#[inline(never)]
fn sfb_stop(why: &str, a: u64, b: u64) -> Option<SimpleFb> {
    serial_println!(
        ":: tegra: JD1-SFB STOP — {} = {:#x} / {:#x}; no scanout inherited (headless) ::",
        why, a, b
    );
    None
}

/// One named per-node rejection witness for the pass-2 `continue` rungs — so a DTB that publishes a
/// simple-framebuffer node the strict resolver then discards says which test the node failed.
#[inline(never)]
fn sfb_reject(node: &[u8], why: &str, a: u64, b: u64) {
    serial_println!(
        ":: tegra: JD1-SFB reject — node {}: {} = {:#x} / {:#x} ::",
        core::str::from_utf8(node).unwrap_or("?"),
        why,
        a,
        b
    );
}

/// JD1 (video): human-readable dump of the DTB display handoff — the twin of `nvdisplay_simplefb`.
/// Read-only RAM walk (NO display MMIO). Prints every `simple-framebuffer` node it finds (geometry,
/// format, memory-region/reg) plus any reserved-memory child whose name mentions fb/framebuffer/
/// display, so a bench boot sees exactly what the firmware published even when the strict resolver
/// rejects it. Every outcome — including every way of finding nothing — is one serial line.
pub fn jd1_dump(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) {
    if dtb_addr == 0 || dtb_size == 0 {
        serial_println!(":: tegra: JD1 — no DTB handed off; cannot survey the scanout handoff ::");
        return;
    }
    let g_lo = dtb_addr >> 30;
    let g_hi = (dtb_addr + dtb_size as u64 - 1) >> 30;
    let mapped = |g: u64| g == 0 || (g < 64 && (ram_gib_mask >> g) & 1 != 0);
    if !mapped(g_lo) || !mapped(g_hi) {
        serial_println!(":: tegra: JD1 — DTB GiB unmapped (mask {:#x}); skip survey ::", ram_gib_mask);
        return;
    }
    let blob = unsafe { core::slice::from_raw_parts(dtb_addr as *const u8, dtb_size) };
    let Some(fdt) = Fdt::new(blob) else {
        serial_println!(":: tegra: JD1 — bad DTB header; skip survey ::");
        return;
    };

    let mut paths = [[0u8; MAX_PATH]; MAX_FB_NODES];
    let mut lens = [0usize; MAX_FB_NODES];
    let mut n = 0usize;
    fdt.for_each_prop(|e| {
        if n >= MAX_FB_NODES || e.name != b"compatible" {
            return;
        }
        let end = (e.val_off + e.val_len).min(blob.len());
        if blob[e.val_off..end].windows(18).any(|q| q == b"simple-framebuffer") {
            let l = e.path.len().min(MAX_PATH);
            let dup = (0..n).any(|i| lens[i] == l && &paths[i][..l] == &e.path[..l]);
            if !dup {
                paths[n][..l].copy_from_slice(&e.path[..l]);
                lens[n] = l;
                n += 1;
            }
        }
    });
    for i in 0..n {
        let node = &paths[i][..lens[i]];
        let wp = fdt.prop_at(node, b"width");
        let hp = fdt.prop_at(node, b"height");
        let sp = fdt.prop_at(node, b"stride");
        let mr = fdt.prop_at(node, b"memory-region");
        let rg = fdt.prop_at(node, b"reg");
        let fmtp = fdt.prop_at(node, b"format");
        let mut fb = [b' '; 16];
        let mut fl = 0usize;
        'x: for wi in 0..fmtp.n {
            for b in fmtp.words[wi].to_be_bytes() {
                if b == 0 || fl >= 16 {
                    break 'x;
                }
                fb[fl] = b;
                fl += 1;
            }
        }
        serial_println!(
            ":: tegra: JD1 — simple-fb {}: {}x{} stride={} fmt='{}' memory-region{} reg{} ::",
            core::str::from_utf8(node).unwrap_or("?"),
            if wp.n >= 1 { wp.words[0] } else { 0 },
            if hp.n >= 1 { hp.words[0] } else { 0 },
            if sp.n >= 1 { sp.words[0] } else { 0 },
            core::str::from_utf8(&fb[..fl]).unwrap_or("?"),
            w(&mr),
            w(&rg),
        );
    }
    if n == 0 {
        serial_println!(
            ":: tegra: JD1 — no simple-framebuffer node in DTB (SIMPLEFB handoff not published here?) ::"
        );
    }
    // Reserved-memory fb/display carveouts — the DRAM the scanout is backed by (cross-check).
    let mut resv = 0u32;
    fdt.for_each_prop(|e| {
        if e.name == b"reg" && e.depth == 3 && resv < 6 {
            let p = e.path;
            if p.len() > 16 && &p[..16] == b"/reserved-memory" {
                let name = &p[17.min(p.len())..];
                let interesting = name.windows(2).any(|q| q == b"fb")
                    || name.windows(7).any(|q| q == b"display")
                    || name.windows(11).any(|q| q == b"framebuffer");
                if interesting {
                    let words = PropWords::capture(blob, e.val_off, e.val_len);
                    serial_println!(
                        ":: tegra: JD1 — resv {} reg{} ::",
                        core::str::from_utf8(p).unwrap_or("?"),
                        w(&words),
                    );
                    resv += 1;
                }
            }
        }
    });
}

/// JD1 (video, DC-probe fallback only): the Tegra234 display-controller block base+size from the
/// DTB `display@…` node (compatible `nvidia,tegra234-display`, reg ~`0x1380_0000` on L4T). Used ONLY
/// by the default-off `display_tegra::JD1_DC_PROBE` register survey; the primary handoff is the
/// `simple-framebuffer` reg above. Verify-don't-assume — resolved from the live DTB, never hardcoded,
/// so the survey's read-only sweep stays inside the block's own decoded aperture. `reg` on the
/// wrapper is (addr:2, size:2) cells. `None` = node/reg absent.
pub fn nvdisplay_base(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) -> Option<(u64, u64)> {
    if dtb_addr == 0 || dtb_size == 0 {
        return None;
    }
    let g_lo = dtb_addr >> 30;
    let g_hi = (dtb_addr + dtb_size as u64 - 1) >> 30;
    let mapped = |g: u64| g == 0 || (g < 64 && (ram_gib_mask >> g) & 1 != 0);
    if !mapped(g_lo) || !mapped(g_hi) {
        return None;
    }
    let blob = unsafe { core::slice::from_raw_parts(dtb_addr as *const u8, dtb_size) };
    let fdt = Fdt::new(blob)?;
    let mut path = [0u8; MAX_PATH];
    let mut plen = 0usize;
    fdt.for_each_prop(|e| {
        // Match the node name component "display@" (not the "display-hub@" sibling, whose name has
        // "display-" not "display@").
        if plen == 0 && e.path.windows(8).any(|q| q == b"display@") {
            let l = e.path.len().min(MAX_PATH);
            path[..l].copy_from_slice(&e.path[..l]);
            plen = l;
        }
    });
    if plen == 0 {
        return None;
    }
    let reg = fdt.prop_at(&path[..plen], b"reg");
    if reg.n < 4 {
        return None;
    }
    let base = ((reg.words[0] as u64) << 32) | reg.words[1] as u64;
    let size = ((reg.words[2] as u64) << 32) | reg.words[3] as u64;
    if base == 0 {
        return None;
    }
    Some((base, size))
}

/// ORIN-SMP-3: enumerate the REAL present CPU cores from the firmware DTB `/cpus` node — the ONLY
/// presence oracle for the tegra SMP kick-off (arch_arm64.md §ORIN-SMP-3, RIDER 1). Fills `out` with
/// each `cpu@…` node's packed affinity {Aff3[31:24],Aff2[23:16],Aff1[15:8],Aff0[7:0]} (the form
/// `gic::this_affinity()` / `smp_virt` use) and returns the count.
///
/// **Why `/cpus`, not `AFFINITY_INFO` or the redistributor walk (the hard-won oracle discrepancy,
/// silicon-measured on the 6-core Nano — RIDER 3):** `AFFINITY_INFO` answers a *valid* state (0/1/2)
/// for 12 affinity slots (the die's full 12-core layout) even though only 6 are fused-in — it is a
/// firmware topology table that cannot be trusted for presence. The GIC-600 exposes 8 redistributor
/// frames (the die's core slots), also more than the 6 real cores. Only the DTB `/cpus` node, which
/// NVIDIA's UEFI populates from the fused SKU, names exactly the 6 present cores. Trusting the wrong
/// oracle re-opens the JM5 attempt-1 wall (a `CPU_ON` to a fuse-disabled phantom is a fatal RAS UE).
/// So the tegra kick-off's target list is produced HERE, from the FDT walk alone.
///
/// A CPU's `reg` on `/cpus` is its MPIDR affinity in MPIDR layout (Aff3 at bits[39:32]); the number of
/// cells is `/cpus/#address-cells` (1 on Tegra234 — a single 32-bit affinity; 2 is honored for a
/// 64-bit affinity). Read-only RAM walk, pre-heap, EL2; a malformed/unmapped blob yields 0 (the caller
/// STOPs rather than guessing). Bounded by `out.len()`.
// Available to the ORIN-SMP-3 kick-off (`tegrasmp`) AND the ORIN-SMP-4 bisect probe (`smpprobe`), which
// sources its single woken-core target from the same `/cpus` oracle (RIDER 5). Off both features (the
// default tegra image) it compiles out entirely, so baseline byte-identity is unaffected.
#[cfg(any(feature = "tegrasmp", feature = "smpprobe"))]
pub fn cpu_affinities(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64, out: &mut [u32]) -> usize {
    if dtb_addr == 0 || dtb_size == 0 {
        return 0;
    }
    let g_lo = dtb_addr >> 30;
    let g_hi = (dtb_addr + dtb_size as u64 - 1) >> 30;
    let mapped = |g: u64| g == 0 || (g < 64 && (ram_gib_mask >> g) & 1 != 0);
    if !mapped(g_lo) || !mapped(g_hi) {
        return 0;
    }
    let blob = unsafe { core::slice::from_raw_parts(dtb_addr as *const u8, dtb_size) };
    let Some(fdt) = Fdt::new(blob) else {
        return 0;
    };
    // /cpus #address-cells (DT default for /cpus is 1 — a single-cell MPIDR affinity).
    let ac = {
        let p = fdt.prop_at(b"/cpus", b"#address-cells");
        if p.n >= 1 && p.words[0] == 2 { 2 } else { 1 }
    };
    let mut n = 0usize;
    fdt.for_each_prop(|e| {
        if n >= out.len() || e.name != b"reg" || e.depth != 3 {
            return;
        }
        // A DIRECT `/cpus/cpu@…` child only: path is exactly "/cpus/<leaf>" with leaf = "cpu@…" and
        // no deeper '/' (so a `cpu-map`/thermal grandchild's reg can never masquerade as a core).
        let p = e.path;
        if p.len() <= 6 || &p[..6] != b"/cpus/" {
            return;
        }
        let leaf = &p[6..];
        if leaf.len() < 4 || &leaf[..4] != b"cpu@" || leaf.iter().any(|&b| b == b'/') {
            return;
        }
        let words = PropWords::capture(blob, e.val_off, e.val_len);
        let mpidr = if ac == 2 && words.n >= 2 {
            ((words.words[0] as u64) << 32) | words.words[1] as u64
        } else if words.n >= 1 {
            words.words[0] as u64
        } else {
            return;
        };
        // MPIDR layout (Aff3 @ bits[39:32]) -> packed GICR-contiguous form (Aff3 @ bits[31:24]).
        let packed = ((((mpidr >> 32) & 0xff) as u32) << 24) | (mpidr as u32 & 0x00ff_ffff);
        out[n] = packed;
        n += 1;
    });
    n
}

// =================================================================================================
// JD1-DC — the DTB resource-id reader, GENERALISED off its one node name. `jd1dc`, DEFAULT OFF.
// =================================================================================================
//
// WHAT THIS IS. `xusb_ids` above is a perfectly good `clocks`/`resets`/`power-domains` reader that
// happens to have ONE thing hardcoded to XUSB: the node-name literal `b"usb@3610000"` it matches on.
// The JD1-DC nvdisplay probe needs exactly the same three lists off a DIFFERENT node
// (`display@13800000`), because its MRQ_PG guard cannot prove a power domain is ON without the
// domain's id, and the id lives in the firmware's own tree (verify-don't-assume — never a header
// guess, the JB1c rule). So the reader is parameterised by node name here, and `xusb_ids` routes
// THROUGH it on an armed build.
//
// WHY IT IS AN APPENDED, FEATURE-GATED COPY OF THE WALK RATHER THAN AN IN-PLACE EDIT OF `xusb_ids`.
// The arc's hard constraint is that the DISARMED jetson image is byte-identical to baseline, and
// that constraint is stronger than it first looks: ANY edit to source the disarmed build compiles
// changes the image. Not just code — the two `ids_stop` message literals in `xusb_ids` say
// "usb@3610000", and a generalised reader must not print that when it was asked for a display node,
// so an in-place parameterisation necessarily rewrites .rodata. Worse, inserting or deleting a
// LINE anywhere above an existing panic site in this file shifts that site's `core::panic::Location`
// (file/line/col is baked into the image), which moves bytes for free. Hence the shape below: the
// generic walk is `#[cfg(feature = "jd1dc")]` and lives at the file TAIL where it shifts nothing,
// and the single edit to `xusb_ids` is a `#[cfg]`-erased statement APPENDED to its existing opening
// line. Knob off => not one byte of this block, and not one moved line, reaches the image.
//
// THE ONE DELIBERATE BEHAVIOURAL DIFFERENCE from `xusb_ids`: the "found the node but it carries
// nothing" abandon also requires `n_pds == 0`. `xusb_ids` abandons on `n_clocks == 0 && n_resets == 0`
// because for XUSB a node with no clocks and no resets cannot be ungated — but a display node whose
// only BPMP resource in this firmware's tree is `power-domains` is EXACTLY the node the guard wants,
// and abandoning it would refuse the probe for the wrong reason. For `usb@3610000` the two
// conditions cannot disagree (that node carries nine clocks — JB5, metal-proven), which is why
// routing `xusb_ids` through here is safe.
//
// FALL-THROUGH, NOT REPLACEMENT. `xusb_ids`'s armed-build delegation returns only on `Some`: if this
// reader ever answered `None` where the original body would have answered `Some`, the original body
// still runs and the XUSB ungate is unharmed. An armed JD1-DC flight therefore cannot lose its
// keyboard to a bug in this function — the worst case is one extra STOP line on the wire. That
// fall-through is also the self-test: on an armed boot the `JB1c — XUSB ids from DTB: 9 clocks,
// 4 resets, 1 power-domains` line is produced BY this reader, against a node whose answer is known.

/// JD1-DC: `xusb_ids`'s walk with the node name lifted into a parameter — read one DTB node's
/// `clocks` / `resets` / `power-domains` id lists (the odd-index word of each [phandle, id] pair).
///
/// `node_name` is matched as a SUBSTRING of the node path, identically to `xusb_ids` and to
/// `nvdisplay_base` — so passing `b"display@"` here resolves the SAME node `nvdisplay_base` resolves
/// (same predicate, same `for_each_prop` order, first match wins). That identity is load-bearing for
/// the probe: the power domain the guard proves ON must be the domain that owns the aperture the
/// sweep reads, and matching on the same string is what makes that true by construction rather than
/// by assumption.
///
/// Read-only RAM walk — no MMIO, no allocation. Every abandon names itself on the wire
/// (`JD1-DC-IDS STOP`) with the node it was asked for.
#[cfg(feature = "jd1dc")]
pub fn node_ids(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64, node_name: &[u8]) -> Option<XusbIds> {
    if node_name.is_empty() || node_name.len() > MAX_PATH {
        // `windows(0)` panics and a name longer than a path can never match; refuse both up front.
        return node_ids_stop(node_name, "node name length out of range (len / MAX_PATH)", node_name.len() as u64, MAX_PATH as u64);
    }
    if dtb_addr == 0 || dtb_size == 0 {
        return node_ids_stop(node_name, "no DTB handed off (addr / size)", dtb_addr, dtb_size as u64);
    }
    let g_lo = dtb_addr >> 30;
    let g_hi = (dtb_addr + dtb_size as u64 - 1) >> 30;
    let mapped = |g: u64| g == 0 || (g < 64 && (ram_gib_mask >> g) & 1 != 0);
    if !mapped(g_lo) || !mapped(g_hi) {
        return node_ids_stop(node_name, "DTB GiB unmapped (GiB lo / RAM-GiB-mask)", g_lo, ram_gib_mask);
    }
    let blob = unsafe { core::slice::from_raw_parts(dtb_addr as *const u8, dtb_size) };
    let Some(fdt) = Fdt::new(blob) else {
        return node_ids_stop(
            node_name,
            "bad DTB header (magic/bounds) at Fdt::new; first two words",
            be32(blob, 0).unwrap_or(0) as u64,
            be32(blob, 4).unwrap_or(0) as u64,
        );
    };
    let mut path = [0u8; MAX_PATH];
    let mut plen = 0usize;
    fdt.for_each_prop(|e| {
        if plen == 0 && e.path.windows(node_name.len()).any(|w| w == node_name) {
            let l = e.path.len().min(MAX_PATH);
            path[..l].copy_from_slice(&e.path[..l]);
            plen = l;
        }
    });
    if plen == 0 {
        return node_ids_stop(node_name, "no node whose path contains this name", 0, 0);
    }
    let node = &path[..plen];
    // [phandle, id] pairs -> collect the odd-index words. Verbatim from `xusb_ids`.
    let pick = |p: &PropWords, out: &mut [u32], n: &mut usize| {
        let mut i = 1;
        while i < p.n && *n < out.len() {
            out[*n] = p.words[i];
            *n += 1;
            i += 2;
        }
    };
    let mut ids = XusbIds { clocks: [0; 9], n_clocks: 0, resets: [0; 4], n_resets: 0, pds: [0; 4], n_pds: 0 };
    let clocks = fdt.prop_at(node, b"clocks");
    let resets = fdt.prop_at(node, b"resets");
    let pds = fdt.prop_at(node, b"power-domains");
    pick(&clocks, &mut ids.clocks, &mut ids.n_clocks);
    pick(&resets, &mut ids.resets, &mut ids.n_resets);
    pick(&pds, &mut ids.pds, &mut ids.n_pds);
    if ids.n_clocks == 0 && ids.n_resets == 0 && ids.n_pds == 0 {
        return node_ids_stop(
            node_name,
            "node found but carries NO clocks, NO resets and NO power-domains (raw prop cell counts clocks.n / resets.n)",
            clocks.n as u64,
            resets.n as u64,
        );
    }
    // Two lines rather than one with conditional fragments: a format that splices "" and 0 together
    // when there are no power-domains reads as `...0 power-domains0 ::` on the wire, and a capture a
    // decision rests on may not contain a number that means nothing.
    serial_println!(
        ":: tegra: JD1-DC-IDS — node '{}' (path '{}'): {} clocks, {} resets, {} power-domains ::",
        core::str::from_utf8(node_name).unwrap_or("?"),
        core::str::from_utf8(node).unwrap_or("?"),
        ids.n_clocks,
        ids.n_resets,
        ids.n_pds,
    );
    for i in 0..ids.n_pds {
        serial_println!(":: tegra: JD1-DC-IDS —   power-domain[{}] id = {} ({:#x}) ::", i, ids.pds[i], ids.pds[i]);
    }
    Some(ids)
}

/// One named JD1-DC-IDS abandon witness + the `None` the rung returns (tail-helper convention: each
/// abandon stays a single `return` line, so no call-site `Location` moves). Names the node it was
/// asked for — the whole reason the generalised reader could not reuse `ids_stop`, whose two
/// messages say "usb@3610000" and whose text may not change without moving the disarmed image.
#[cfg(feature = "jd1dc")]
#[inline(never)]
fn node_ids_stop(node_name: &[u8], why: &str, a: u64, b: u64) -> Option<XusbIds> {
    serial_println!(
        ":: tegra: JD1-DC-IDS STOP — node '{}': {} = {:#x} / {:#x}; resource ids unresolved ::",
        core::str::from_utf8(node_name).unwrap_or("?"),
        why,
        a,
        b,
    );
    None
}
