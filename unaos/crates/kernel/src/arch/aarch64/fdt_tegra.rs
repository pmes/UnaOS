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
/// Most raw u32 words we capture from one property (the XUSB `clocks` list is 8 [phandle, id]
/// pairs = 16 words — keep headroom above that).
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

/// Resolve the JB1b geometry silently (the JB1a dump is the human-readable twin). `None` = any
/// piece missing or shaped unexpectedly — the caller prints and stops rather than guessing.
pub fn bpmp_geometry(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) -> Option<BpmpGeom> {
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
        return None;
    }
    let bp = &bpmp_path[..bpmp_len];
    let mboxes = fdt.prop_at(bp, b"mboxes");
    let shmem = fdt.prop_at(bp, b"shmem");
    if mboxes.n < 3 || shmem.n < 2 {
        return None;
    }

    // A shmem phandle target: child reg [offset, size] + parent reg [hi lo hi lo] -> absolute PA.
    let resolve_shmem = |ph: u32| -> Option<u64> {
        let mut buf = [0u8; MAX_PATH];
        let n = fdt.path_of_phandle(ph, &mut buf);
        if n == 0 {
            return None;
        }
        let node = &buf[..n];
        let reg = fdt.prop_at(node, b"reg");
        let preg = fdt.prop_at(parent(node), b"reg");
        if reg.n < 1 || preg.n < 2 {
            return None;
        }
        let parent_base = ((preg.words[0] as u64) << 32) | preg.words[1] as u64;
        Some(parent_base + reg.words[0] as u64)
    };
    let shmem_tx = resolve_shmem(shmem.words[0])?;
    let shmem_rx = resolve_shmem(shmem.words[1])?;

    // The HSP node behind mboxes[0]: reg [hi lo hi lo] -> base.
    let mut hbuf = [0u8; MAX_PATH];
    let hn = fdt.path_of_phandle(mboxes.words[0], &mut hbuf);
    if hn == 0 {
        return None;
    }
    let hreg = fdt.prop_at(&hbuf[..hn], b"reg");
    if hreg.n < 2 {
        return None;
    }
    let hsp_base = ((hreg.words[0] as u64) << 32) | hreg.words[1] as u64;
    Some(BpmpGeom { shmem_tx, shmem_rx, hsp_base, db_master: mboxes.words[2] })
}

/// JB1c: the XUSB host node's BPMP resource IDs, read off the firmware DTB (never a header
/// guess). `clocks`/`resets`/`power-domains` are [phandle, id] pairs — we keep the ids.
pub struct XusbIds {
    pub clocks: [u32; 8],
    pub n_clocks: usize,
    pub resets: [u32; 4],
    pub n_resets: usize,
    pub pds: [u32; 4],
    pub n_pds: usize,
}

/// Find the node whose name contains "usb@3610000" and read its BPMP resource id lists.
pub fn xusb_ids(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) -> Option<XusbIds> {
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
        return None;
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
        clocks: [0; 8],
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
        return None;
    }
    Some(ids)
}

/// JB2b: does the firmware DTB mark the XUSB host node `dma-coherent`? A DT boolean is presence
/// (`val_len == 0`), so `prop_at(..).found` is the test. `Some(true)` = the controller's DMA snoops
/// the CPU caches (Normal-WB rings need no cache maintenance — the Linux tegra234.dtsi stance);
/// `Some(false)` = the prop is absent from this firmware's tree (verify-don't-assume: the attach
/// still proceeds, and a stale-ring enumeration stall becomes the diagnosis); `None` = the node or
/// the DTB itself was unresolvable. Same node match as `xusb_ids`.
pub fn xusb_dma_coherent(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) -> Option<bool> {
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
        return None;
    }
    Some(fdt.prop_at(&path[..plen], b"dma-coherent").found)
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
