//! CHARTER: Kernel — driver (KEPLERGOLD rmbp-ledger B656, R119 + R127: the GK107 GR golden context image, generated once after the FECS ext start, behind `tests kgr`)
//!
//! KEPLERGOLD — the rung after KEPLERUCODE (B637). Nouveau never names a GR object before a golden context image
//! exists (`gf100_gr_chan_new` generates it once, `gf100_grctx_generate`, ctxgf100.c:1436-1558, on top of
//! `gk104_gr_init`). This file is that generator for gk104 (the GK107 is a gk104_gr part, gr/gk104.c `gk104_gr`),
//! written from nouveau's FACTS (R95 §2 / R110; nouveau MIT, Copyright 2013 Red Hat Inc., Ben Skeggs). Declared from
//! `kepler_fifo.rs` as `kepler_fifo::ctx` (`#[path]`, gpu/mod.rs untouched — the gr4/ucode pattern) under exactly
//! `kgr`'s gate, and called only from `kgr::tests_kgr` after `ucode::load_and_start` answered the image size and
//! before any channel is bound (R80: nothing at boot).
//!
//! CITATIONS. The session that wrote this could read nouveau and envytools only through a question-answering fetch
//! (curl/gh/MCP refused; KEPLERGOLD.md §3): every fact below is cited by FUNCTION or ARRAY name in
//! docs/dev/evidence/keplergold/RESUME.md §4 (and KEPLERGOLD.md §2), NOT by file:line — say so wherever cited.
//! The keplerucode citation `gf100_grctx_generate = ctxgf100.c:1436-1558` (keplerucode.md §4) still stands.
//!
//! THE REGISTER LISTS (`Row`/`List` below) are the arc's "tables with file:line per row". Three are in hand
//! (gf119 prop_0, gpc_unk_1, crstr_0 — RESUME.md §4.4); the other lists are NAMED, ROW-COUNTED, EMPTY slots
//! (RESUME.md §4.3, counts the answering model's, re-counted at transcription). STAGE `lists` refuses before ANY
//! GPU write while a slot is short — the generator never runs a partial table. Filling a slot is a mechanical
//! transcription of ctxgk104.c / ctxgf100.c / ctxgf117.c with file:line per row.
//!
//! PRESUMED, NOT PORTED: `gk104_gr_init` (`gf100_gr_init` with gk104's hooks and `gk104_gr_pack_mmio`, RESUME.md
//! §4.5) runs before nouveau's generator; this tree runs only `gf100_gr_init_ctxctl_ext` (kepler_ucode.rs). STAGE
//! `presume` refuses `gr-init-unported` until that port lands (its own rung; the lists it needs are slots too).
//!
//! THE FECS RESET (RESUME.md §4.1 step 3, the open question): `gf100_gr_fecs_reset` writes 0x409614 = 0x70, then
//! sets 0x700 — the context-switch ENGINE/CONTEXT reset controls, not the falcon's CPUCTL (0x409100) — and nothing in
//! nouveau reloads IMEM or restarts the falcon after it: the generator sends method 0x3 (bind pointer) to the
//! firmware a few writes later (step 8), which only a running firmware answers. So the firmware SURVIVES the reset
//! (an inference from nouveau's order, unverified on metal): this file does NOT re-run kepler_ucode's start; it reads
//! FECS CPUCTL after the reset and refuses `fecs-halted-after-reset` if bit 4 (HALTED) is set.
//!
//! MEMORY. Identity VM (GPU VA == VRAM offset, KF24) through the kgr channel's PD/PT, which `kgr` extends over the
//! GOLD window (`GOLD_SPAN`, reserved in `kgr::arm`). The generator binds the kgr channel's own instance block (nouveau
//! uses a scratch one; the bind is the same method) and zeroes its 0x210/0x214 after the save (step 12). Then the
//! per-channel bind (`gf100_gr_chan_bind`, RESUME.md §4.6): the golden image copied into the channel's context, its
//! header words written, mmio list EMPTY (nouveau's list re-applies pagepool/bundle/attrib per channel; here they are
//! the golden generation's own buffers in the same VM, captured by the save — a design inference, unverified), and
//! the instance block's 0x210/0x214 = channel context | 4 (fifo/gk104.c engine ctx, kgr's INST_GR_CTX). Only then
//! does `ucode::golden()` read `Some`, which is what `kgr` gates the SET_OBJECT on.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::super::kepler::{classify_fecs_word, fecs_read, fecs_write, mmio_read, mmio_write};
use super::ucode;

// ---- the row tables (gf100.h `struct gf100_gr_init { u32 addr; u8 count; u32 pitch; u64 data; }`) ---------------

/// One list row: `count` writes of `data` at `addr + i * pitch` (mmio replay, no masks — `gf100_gr_mmio`).
#[derive(Clone, Copy)]
pub struct Row {
    pub addr: u32,
    pub count: u8,
    pub pitch: u32,
    pub data: u64,
}
const fn r(addr: u32, count: u8, pitch: u32, data: u64) -> Row {
    Row { addr, count, pitch, data }
}

/// A named list: nouveau's array name, its source file, the row count expected (0 = count not yet known — any
/// non-empty transcription is accepted), and the rows.
pub struct List {
    pub name: &'static str,
    pub src: &'static str,
    pub expect: usize,
    pub rows: &'static [Row],
}
const fn l(name: &'static str, src: &'static str, expect: usize, rows: &'static [Row]) -> List {
    List { name, src, expect, rows }
}
/// An untranscribed slot.
const SLOT: &[Row] = &[];

// In hand (RESUME.md §4.4; ctxgf119.c, exported arrays, cited by name).
/// `gf119_grctx_init_prop_0` (8 rows).
const GF119_PROP_0: &[Row] = &[
    r(0x418400, 1, 4, 0x3800_4e00),
    r(0x418404, 1, 4, 0x71e0_ffff),
    r(0x41840c, 1, 4, 0x0000_1008),
    r(0x418410, 1, 4, 0x0fff_0fff),
    r(0x418414, 1, 4, 0x0220_0fff),
    r(0x418450, 6, 4, 0),
    r(0x418468, 1, 4, 1),
    r(0x41846c, 2, 4, 0),
];
/// `gf119_grctx_init_gpc_unk_1` (5 rows).
const GF119_GPC_UNK_1: &[Row] = &[
    r(0x418600, 1, 4, 0x1f),
    r(0x418684, 1, 4, 0xf),
    r(0x418700, 1, 4, 2),
    r(0x418704, 1, 4, 0x80),
    r(0x418708, 3, 4, 0),
];
/// `gf119_grctx_init_crstr_0` (8 rows).
const GF119_CRSTR_0: &[Row] = &[
    r(0x418b00, 1, 4, 6),
    r(0x418b08, 1, 4, 0x0a41_8820),
    r(0x418b0c, 1, 4, 0x0620_80e6),
    r(0x418b10, 1, 4, 0x0203_98a4),
    r(0x418b14, 1, 4, 0x0e62_9062),
    r(0x418b18, 1, 4, 0x0a41_8820),
    r(0x418b1c, 1, 4, 0xe6),
    r(0x418bb8, 1, 4, 0x103),
];

// gk104_grctx's packs, in nouveau's order (ctxgk104.c `gk104_grctx` + packs; RESUME.md §4.3).
pub const HUB: &[List] = &[
    l("gf100_grctx_init_main_0", "ctxgf100.c", 0, SLOT),
    l("gk104_grctx_init_fe_0", "ctxgk104.c", 16, SLOT),
    l("gf100_grctx_init_pri_0", "ctxgf100.c", 0, SLOT),
    l("gk104_grctx_init_memfmt_0", "ctxgk104.c", 21, SLOT),
    l("gk104_grctx_init_ds_0", "ctxgk104.c", 8, SLOT),
    l("gk104_grctx_init_cwd_0", "ctxgk104.c", 2, SLOT),
    l("gk104_grctx_init_pd_0", "ctxgk104.c", 10, SLOT),
    l("gk104_grctx_init_sked_0", "ctxgk104.c", 1, SLOT),
    l("gf100_grctx_init_rstr2d_0", "ctxgf100.c", 0, SLOT),
    l("gk104_grctx_init_scc_0", "ctxgk104.c", 6, SLOT),
    l("gk104_grctx_init_be_0", "ctxgk104.c", 8, SLOT),
];
pub const GPC_0: &[List] = &[
    l("gf100_grctx_init_gpc_unk_0", "ctxgf100.c", 0, SLOT),
    l("gf119_grctx_init_prop_0", "ctxgf119.c", 8, GF119_PROP_0),
    l("gf119_grctx_init_gpc_unk_1", "ctxgf119.c", 5, GF119_GPC_UNK_1),
    l("gk104_grctx_init_setup_0", "ctxgk104.c", 8, SLOT),
    l("gf100_grctx_init_zcull_0", "ctxgf100.c", 0, SLOT),
];
pub const ZCULL: &[List] = &[l("gf100_grctx_pack_zcull", "ctxgf100.c", 0, SLOT)];
pub const GPC_1: &[List] = &[
    l("gf119_grctx_init_crstr_0", "ctxgf119.c", 8, GF119_CRSTR_0),
    l("gk104_grctx_init_gpm_0", "ctxgk104.c", 6, SLOT),
    l("gf100_grctx_init_gcc_0", "ctxgf100.c", 0, SLOT),
];
pub const TPC: &[List] = &[
    l("gf117_grctx_init_pe_0", "ctxgf117.c", 0, SLOT),
    l("gk104_grctx_init_tex_0", "ctxgk104.c", 10, SLOT),
    l("gk104_grctx_init_mpc_0", "ctxgk104.c", 6, SLOT),
    l("gk104_grctx_init_l1c_0", "ctxgk104.c", 2, SLOT),
    l("gk104_grctx_init_sm_0", "ctxgk104.c", 14, SLOT),
];
pub const PPC: &[List] = &[
    l("gk104_grctx_init_pes_0", "ctxgk104.c", 1, SLOT),
    l("gk104_grctx_init_cbm_0", "ctxgk104.c", 3, SLOT),
    l("gf117_grctx_init_wwdx_0", "ctxgf117.c", 0, SLOT),
];
/// The mmio packs in `gf100_grctx_generate_main`'s replay order (RESUME.md §4.2 item 2).
pub const PACKS: &[(&str, &[List])] = &[("hub", HUB), ("gpc_0", GPC_0), ("zcull", ZCULL), ("gpc_1", GPC_1), ("tpc", TPC), ("ppc", PPC)];
/// `gk104_grctx_pack_icmd` (pack type 32: the 0x40020c high word is never written).
pub const ICMD: &[List] = &[l("gk104_grctx_init_icmd_0", "ctxgk104.c", 239, SLOT)];
/// `gk104_grctx_pack_mthd`: (class, list).
pub const MTHD: &[(u32, List)] = &[
    (0xa097, l("gk104_grctx_init_a097_0", "ctxgk104.c", 295, SLOT)),
    (0x902d, l("gf100_grctx_init_902d_0", "ctxgf100.c", 0, SLOT)),
];

/// `gk104_gr_init` (RESUME.md §4.5) is presumed by nouveau's generator and not ported in this tree.
pub const GR_INIT_PORTED: bool = false;

// ---- the GOLD window (offsets from its base; GPU VA == VRAM offset) -------------------------------------------------
/// CB_RESERVED: the image sits 0x80000 into the context allocation (`gf100_grctx_generate`, RESUME.md §4.1 step 6).
const CB_RESERVED: usize = 0x80000;
/// The largest image size (method 0x10's answer) this window holds; a larger answer refuses `ctx-size`.
const MAX_CTX: usize = 0x80000;
const G_CTX: usize = 0x000000; // CB_RESERVED + MAX_CTX
const G_CHAN: usize = 0x100000; // the channel's copy of the golden image (MAX_CTX)
const G_BUNDLE: usize = 0x180000; // bundle_size 0x3000
const G_PAGEPOOL: usize = 0x184000; // pagepool_size 0x8000
const G_ATTRIB: usize = 0x18C000; // attrib_cb, 4 KiB aligned (written >> 12)
/// The window `kgr::arm` reserves and maps.
pub const GOLD_SPAN: usize = 0x240000;
const BUNDLE_SIZE: u32 = 0x3000;
const PAGEPOOL_SIZE: usize = 0x8000;

// ---- gk104 constants (RESUME.md §4.2) -------------------------------------------------------------------------------
const ATTRIB_NR_MAX: u32 = 0x324;
const ALPHA_NR_MAX: u32 = 0x7ff;
const ATTRIB_NR: u32 = 0x218;
const ALPHA_NR: u32 = 0x648;
const BUNDLE_MIN_GPM_FIFO_DEPTH: u32 = 0x180;
const BUNDLE_TOKEN_LIMIT: u32 = 0x600;
const PPC_NR: usize = 1; // gk104_gr .ppc_nr

/// This generator's topology cap (gf100.h GPC_MAX 32 / TPC_MAX_PER_GPC 8; the GK107 reads 1 GPC — see the wire).
const GPC_CAP: usize = 8;
const TPC_CAP: u32 = 8;
const BUDGET_US: u64 = 2_000_000; // nvkm_msec(device, 2000, …)

#[inline]
const fn gpc_unit(g: usize, r: usize) -> usize {
    0x500000 + g * 0x8000 + r
}
#[inline]
const fn ppc_unit(g: usize, m: usize, r: usize) -> usize {
    0x503000 + g * 0x8000 + m * 0x200 + r
}
#[inline]
const fn tpc_unit(g: usize, t: usize, r: usize) -> usize {
    0x504000 + g * 0x8000 + t * 0x800 + r
}

// ---- census (stage `lists`) -------------------------------------------------------------------------------------------
pub struct Census {
    pub lists: usize,
    pub have: usize,
    pub expect: usize,
    pub uncounted: usize,
    pub short: usize,
    pub first_short: Option<&'static str>,
}

fn each_list(mut f: impl FnMut(&List)) {
    for (_, p) in PACKS {
        for li in p.iter() {
            f(li);
        }
    }
    for li in ICMD {
        f(li);
    }
    for (_, li) in MTHD {
        f(li);
    }
}

/// STAGE `lists`: every slot transcribed (non-empty, and equal to its expected count when one is known).
pub fn census() -> Census {
    let mut c = Census { lists: 0, have: 0, expect: 0, uncounted: 0, short: 0, first_short: None };
    each_list(|li| {
        c.lists += 1;
        c.have += li.rows.len();
        if li.expect == 0 {
            c.uncounted += 1;
        } else {
            c.expect += li.expect;
        }
        if li.rows.is_empty() || (li.expect != 0 && li.rows.len() != li.expect) {
            c.short += 1;
            if c.first_short.is_none() {
                c.first_short = Some(li.name);
            }
        }
    });
    c
}

// ---- io ------------------------------------------------------------------------------------------------------------------
struct Io {
    bar0: usize,
    vw: fn(usize, u32),
    vr: fn(usize) -> u32,
    n: usize,
}

impl Io {
    #[inline]
    fn fecs(off: usize) -> bool {
        (0x409000..0x40a000).contains(&off)
    }
    fn wr(&mut self, off: usize, v: u32) {
        self.n += 1;
        if Self::fecs(off) {
            fecs_write(self.bar0, off, v)
        } else {
            unsafe { mmio_write(self.bar0, off, v) }
        }
    }
    fn rd(&self, off: usize) -> u32 {
        if Self::fecs(off) { fecs_read(self.bar0, off) } else { unsafe { mmio_read(self.bar0, off) } }
    }
    fn mask(&mut self, off: usize, m: u32, v: u32) -> u32 {
        let o = self.rd(off);
        self.wr(off, (o & !m) | v);
        o
    }
    /// `gf100_gr_wait_idle`: read 0x400700 (flush); idle when GR is not enabled in PMC (0x200 bit 12) or
    /// PGRAPH_STATUS (0x40060c) bit 0 is clear. Nouveau also requires no FIFO ctxsw in progress — none can be here:
    /// the generator runs before any channel is bound (kgr calls it ahead of `fifo_init`).
    fn wait_idle(&self) -> bool {
        ucode::poll(BUDGET_US, || {
            let _ = self.rd(0x400700);
            let en = self.rd(0x200) & 0x1000;
            let busy = self.rd(0x40060c) & 1;
            if en == 0 || busy == 0 { Some(()) } else { None }
        })
        .is_some()
    }
    fn replay(&mut self, li: &List) -> usize {
        let mut n = 0;
        for row in li.rows {
            for i in 0..row.count as usize {
                self.wr(row.addr as usize + i * row.pitch as usize, row.data as u32);
                n += 1;
            }
        }
        n
    }
}

fn stage(name: &str, rows: usize) {
    serial_println!("[kgr] ctx stage={} rows={}", name, rows);
}
fn refuse(name: &str, rows: usize, why: String) -> Result<u32, String> {
    serial_println!("[kgr] ctx stage={} rows={} -> refused({})", name, rows, why);
    Err(why)
}

// ---- topology (gf100_gr_oneinit + _tiles + _sm_id, RESUME.md / KEPLERGOLD.md §2.0) -------------------------------------
struct Topo {
    gpc_nr: usize,
    rop_nr: u32,
    tpc_nr: [u32; GPC_CAP],
    tpc_total: u32,
    tpc_max: u32,
    ppc_mask: [u32; GPC_CAP],
    ppc_tpc_mask: [u32; GPC_CAP],
    ppc_tpc_nr: [u32; GPC_CAP],
    tile: [u8; 32],
    row_off: u32,
    sm: Vec<(usize, usize)>,
}

fn topology(io: &Io) -> Result<Topo, String> {
    let w = io.rd(0x409604);
    if classify_fecs_word(w) != "VALUE" {
        return Err(format!("r409604={:08X}", w));
    }
    let gpc_nr = (w & 0x1f) as usize;
    if gpc_nr == 0 || gpc_nr > GPC_CAP {
        return Err(format!("gpc_nr={}", gpc_nr));
    }
    let mut t = Topo {
        gpc_nr,
        rop_nr: (w & 0x001f_0000) >> 16,
        tpc_nr: [0; GPC_CAP],
        tpc_total: 0,
        tpc_max: 0,
        ppc_mask: [0; GPC_CAP],
        ppc_tpc_mask: [0; GPC_CAP],
        ppc_tpc_nr: [0; GPC_CAP],
        tile: [0xff; 32],
        row_off: 0,
        sm: Vec::new(),
    };
    for g in 0..gpc_nr {
        let n = io.rd(gpc_unit(g, 0x2608));
        if n > TPC_CAP {
            return Err(format!("tpc_nr[{}]={:08X}", g, n));
        }
        t.tpc_nr[g] = n;
        t.tpc_total += n;
        t.tpc_max = t.tpc_max.max(n);
        // ppc_nr = 1 for gk104: one mask per GPC (GPC_UNIT(g, 0x0c30 + ppc * 4)).
        let m = io.rd(gpc_unit(g, 0x0c30));
        if classify_fecs_word(m) == "POISON" || m == 0xFFFF_FFFF {
            return Err(format!("ppc_tpc_mask[{}]={:08X}", g, m));
        }
        if m != 0 {
            t.ppc_tpc_mask[g] = m;
            t.ppc_mask[g] = 1;
            t.ppc_tpc_nr[g] = (m & 0xff).count_ones();
        }
    }
    if t.tpc_total == 0 {
        return Err(String::from("tpc_total=0"));
    }
    // gf100_gr_oneinit_tiles: screen_tile_row_offset by tpc_total, default the first prime not dividing it.
    t.row_off = match t.tpc_total {
        15 => 0x06,
        14 => 0x05,
        13 => 0x02,
        11 => 0x07,
        10 => 0x06,
        7 | 5 => 0x01,
        3 => 0x02,
        2 | 1 => 0x01,
        n => {
            let primes = [3u32, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53, 59, 61];
            primes.iter().copied().find(|p| n % *p != 0).unwrap_or(0x03)
        }
    };
    // The weighted round-robin: GPCs sorted by tpc_nr, highest first (a stable sort == nouveau's bubble sort).
    let mut map: Vec<usize> = (0..gpc_nr).collect();
    map.sort_by(|a, b| t.tpc_nr[*b].cmp(&t.tpc_nr[*a]));
    let (gn, tm) = (gpc_nr as i64, t.tpc_max as i64);
    let mul = if (gn * tm) & 1 != 0 { 2 } else { 1 };
    let denom = gn * tm * mul;
    let mut frac = [0i64; GPC_CAP];
    let mut run = [0i64; GPC_CAP];
    for i in 0..gpc_nr {
        frac[i] = t.tpc_nr[map[i]] as i64 * gn * mul;
        run[i] = frac[i] + (i as i64 * tm * mul - denom / 2);
    }
    let (mut i, mut guard) = (0usize, 0u32);
    while (i as u32) < t.tpc_total {
        for j in 0..gpc_nr {
            if run[j] * 2 >= denom {
                if i < 32 {
                    t.tile[i] = map[j] as u8;
                }
                i += 1;
                run[j] += frac[j] - denom;
            } else {
                run[j] += frac[j];
            }
        }
        guard += 1;
        if guard > 1024 {
            return Err(String::from("tiles-no-progress"));
        }
    }
    // gf100_gr_oneinit_sm_id: TPC outer, GPC inner.
    for tpc in 0..t.tpc_max as usize {
        for gpc in 0..gpc_nr {
            if (tpc as u32) < t.tpc_nr[gpc] {
                t.sm.push((gpc, tpc));
            }
        }
    }
    Ok(t)
}

// ---- gf100_grctx_generate_main (RESUME.md §4.2) ------------------------------------------------------------------------
fn main_seq(io: &mut Io, t: &Topo, gold: usize) -> Result<usize, String> {
    let n0 = io.n;
    io.wr(0x260, 0); // 1. nvkm_mc_unk260(0)
    // 2. the mmio packs.
    let mut rows = 0;
    for (_, pack) in PACKS {
        for li in pack.iter() {
            rows += io.replay(li);
        }
    }
    stage("mmio", rows);
    // 3.
    if !io.wait_idle() {
        io.wr(0x260, 1);
        return Err(String::from("idle-wall(mmio)"));
    }
    // 4. idle_timeout saved, zeroed.
    let idle_timeout = io.rd(0x404154);
    io.wr(0x404154, 0);
    // 5. pagepool (gk104).
    let pp = (gold + G_PAGEPOOL) as u32;
    io.wr(0x40800c, pp >> 8);
    io.wr(0x408010, 0x8000_0000);
    io.wr(0x419004, pp >> 8);
    io.wr(0x419008, 0);
    io.wr(0x4064cc, 0x8000_0000);
    // 6. bundle (gk104).
    let bv = (gold + G_BUNDLE) as u32;
    io.wr(0x408004, bv >> 8);
    io.wr(0x408008, 0x8000_0000 | (BUNDLE_SIZE >> 8));
    io.wr(0x418808, bv >> 8);
    io.wr(0x41880c, 0x8000_0000 | (BUNDLE_SIZE >> 8));
    io.wr(0x4064c8, (BUNDLE_MIN_GPM_FIFO_DEPTH.min(BUNDLE_SIZE / 0x20) << 16) | BUNDLE_TOKEN_LIMIT);
    // 7. attrib_cb.
    let av = (gold + G_ATTRIB) as u32;
    io.wr(0x418810, 0x8000_0000 | (av >> 12));
    io.wr(0x419848, 0x1000_0000 | (av >> 12));
    // 8. attrib (gf117_grctx_generate_attrib).
    io.wr(0x405830, (ATTRIB_NR << 16) | ALPHA_NR);
    io.wr(0x4064c4, ((ALPHA_NR / 4) << 16) | 0xffff);
    let (mut bo, mut ao) = (0u32, ATTRIB_NR_MAX * t.tpc_total);
    for g in 0..t.gpc_nr {
        for p in 0..PPC_NR {
            if t.ppc_mask[g] & (1 << p) == 0 {
                continue;
            }
            let n = t.ppc_tpc_nr[g];
            let (a, b, o) = (ALPHA_NR * n, ATTRIB_NR * n, ppc_unit(g, p, 0));
            io.wr(o + 0xc0, (1 << 28) | (b << 16) | bo);
            bo += ATTRIB_NR_MAX * n;
            io.wr(o + 0xe4, (a << 16) | ao);
            ao += ALPHA_NR_MAX * n;
        }
    }
    stage("buffers", io.n - n0 - rows - 1);
    // 9. patch_ltc (gk104): read back, written unchanged.
    let (a, b) = (io.rd(0x17e91c), io.rd(0x17e920));
    io.wr(0x17e91c, a);
    io.wr(0x17e920, b);
    // 10. unkn (gk104).
    io.mask(0x418c6c, 1, 1);
    io.mask(0x41980c, 0x10, 0x10);
    io.mask(0x41be08, 4, 4);
    io.mask(0x4064c0, 0x8000_0000, 0x8000_0000);
    io.mask(0x405800, 0x0800_0000, 0x0800_0000);
    io.mask(0x419c00, 8, 8);
    // 11. floorsweep.
    let f0 = io.n;
    floorsweep(io, t);
    stage("floorsweep", io.n - f0);
    // 12.
    if !io.wait_idle() {
        io.wr(0x404154, idle_timeout);
        io.wr(0x260, 1);
        return Err(String::from("idle-wall(floorsweep)"));
    }
    // 13. ICMD.
    let i0 = io.n;
    let ic = icmd(io);
    stage("icmd", io.n - i0);
    if let Err(e) = ic {
        io.wr(0x404154, idle_timeout);
        io.wr(0x260, 1);
        return Err(e);
    }
    // 14.
    io.wr(0x404154, idle_timeout);
    // 15. MTHD.
    let m0 = io.n;
    for (class, li) in MTHD {
        let ctrl = 0x8000_0000 | class;
        let mut last: Option<u32> = None;
        for row in li.rows {
            let d = row.data as u32;
            if last != Some(d) {
                io.wr(0x40448c, d);
                last = Some(d);
            }
            for i in 0..row.count as u32 {
                io.wr(0x404488, ctrl | ((row.addr + i * row.pitch) << 14));
            }
        }
    }
    stage("mthd", io.n - m0);
    // 16. 17. (gk104's r418800 masks are inside `if (0)`: nothing.)
    io.wr(0x260, 1);
    // 18.
    if !io.wait_idle() {
        return Err(String::from("idle-wall(main)"));
    }
    Ok(io.n - n0)
}

fn floorsweep(io: &mut Io, t: &Topo) {
    for (s, &(g, tp)) in t.sm.iter().enumerate() {
        let s = s as u32;
        io.wr(tpc_unit(g, tp, 0x698), s);
        io.wr(tpc_unit(g, tp, 0x4e8), s);
        io.wr(gpc_unit(g, 0x0c10 + tp * 4), s);
        io.wr(tpc_unit(g, tp, 0x088), s);
    }
    for g in 0..t.gpc_nr {
        io.wr(gpc_unit(g, 0x0c08), t.tpc_nr[g]);
        io.wr(gpc_unit(g, 0x0c8c), t.tpc_nr[g]);
    }
    // gf100_gr_init_num_tpc_per_gpc(pd=false, ds=true) then (true, false): 8 GPCs per word, 4 words.
    for (pd, ds) in [(false, true), (true, false)] {
        let mut gpc = 0usize;
        for i in 0..4usize {
            let mut data = 0u32;
            let mut j = 0;
            while j < 8 && gpc < t.gpc_nr {
                data |= t.tpc_nr[gpc] << (j * 4);
                j += 1;
                gpc += 1;
            }
            if pd {
                io.wr(0x406028 + i * 4, data);
            }
            if ds {
                io.wr(0x405870 + i * 4, data);
            }
        }
    }
    // rop_mapping (gf117).
    let mut data = [0u32; 6];
    for i in 0..32 {
        data[i / 6] |= ((t.tile[i] & 7) as u32) << ((i % 6) * 5);
    }
    let mut ntpcv = t.tpc_total;
    let mut shift = 0u32;
    while ntpcv & (1 << 4) == 0 {
        ntpcv <<= 1;
        shift += 1;
    }
    let d20 = (ntpcv << 16) | (shift << 21) | (((1u32 << 5) % ntpcv) << 24);
    let mut d21 = 0u32;
    for i in 1..=5u32 {
        d21 |= ((1u32 << (i + 5)) % ntpcv) << ((i - 1) * 5);
    }
    let head = (t.tpc_total << 8) | t.row_off;
    io.wr(0x418bb8, head);
    for i in 0..6 {
        io.wr(0x418b08 + i * 4, data[i]);
    }
    io.wr(0x41bfd0, head | d20);
    io.wr(0x41bfe4, d21);
    for i in 0..6 {
        io.wr(0x41bf00 + i * 4, data[i]);
    }
    io.wr(0x4078bc, head);
    for i in 0..6 {
        io.wr(0x40780c + i * 4, data[i]);
    }
    // alpha_beta_tables (gk104).
    for i in 0..32u32 {
        let mut atarget = (t.tpc_total * i / 32).max(1);
        let mut btarget = t.tpc_total - atarget;
        let mut alpha = atarget < btarget;
        let (mut amask, mut bmask) = (0u64, 0u64);
        for g in 0..t.gpc_nr {
            for _p in 0..PPC_NR {
                let mut ppc_tpcs = t.ppc_tpc_nr[g];
                let (abits, bbits);
                if alpha {
                    abits = atarget.min(ppc_tpcs);
                    bbits = btarget.min(ppc_tpcs - abits);
                } else {
                    bbits = btarget.min(ppc_tpcs);
                    abits = atarget.min(ppc_tpcs - bbits);
                }
                let mut pmask = t.ppc_tpc_mask[g] & 0xff;
                while ppc_tpcs > abits {
                    pmask &= pmask.wrapping_sub(1);
                    ppc_tpcs -= 1;
                }
                amask |= (pmask as u64) << (g * 8);
                bmask |= (((t.ppc_tpc_mask[g] & 0xff) ^ pmask) as u64) << (g * 8);
                atarget -= abits.min(atarget);
                btarget -= bbits.min(btarget);
                if abits > 0 || bbits > 0 {
                    alpha = !alpha;
                }
            }
        }
        let mut j = 0usize;
        while j < t.gpc_nr {
            io.wr(0x406800 + i as usize * 0x20 + j, amask as u32);
            io.wr(0x406c00 + i as usize * 0x20 + j, bmask as u32);
            amask >>= 32;
            bmask >>= 32;
            j += 4;
        }
    }
    // dist_skip_table (gf117), gpc_tpc_nr (gk104), r419f78 (gk104).
    for i in 0..8 {
        io.wr(0x4064d0 + i * 4, 0);
    }
    io.wr(0x405b00, (t.tpc_total << 8) | t.gpc_nr as u32);
    io.mask(0x419f78, 0x9, 0);
}

/// `gf100_gr_icmd` over gk104_grctx_pack_icmd.
fn icmd(io: &mut Io) -> Result<(), String> {
    io.wr(0x400208, 0x8000_0000);
    let mut last: Option<u32> = None;
    for li in ICMD {
        for row in li.rows {
            let d = row.data as u32;
            if last != Some(d) {
                io.wr(0x400204, d);
                last = Some(d);
            }
            for i in 0..row.count as u32 {
                let addr = row.addr + i * row.pitch;
                io.wr(0x400200, addr as u32);
                if addr & 0xffff == 0xe100 && !io.wait_idle() {
                    io.wr(0x400208, 0);
                    return Err(format!("icmd-wall(idle addr={:#x})", addr));
                }
                let ok = ucode::poll(BUDGET_US, || if io.rd(0x400700) & 4 == 0 { Some(()) } else { None }).is_some();
                if !ok {
                    io.wr(0x400208, 0);
                    return Err(format!("icmd-wall(addr={:#x})", addr));
                }
            }
        }
    }
    io.wr(0x400208, 0);
    Ok(())
}

/// A FECS method through the firmware's mailbox (`gf100_gr_fecs_bind_pointer` / `_wfi_golden_save` shape):
/// clear `clear` in 0x409800, data -> 0x409500, method -> 0x409504, poll 0x409800 for `fail` (-EIO) or `ok`.
fn fecs_method(io: &mut Io, clear: u32, data: u32, mthd: u32, ok: u32, fail: u32) -> Result<(), &'static str> {
    io.mask(0x409800, clear, 0);
    io.wr(0x409500, data);
    io.wr(0x409504, mthd);
    match ucode::poll(BUDGET_US, || {
        let s = io.rd(0x409800);
        if s & fail != 0 {
            Some(false)
        } else if s & ok != 0 {
            Some(true)
        } else {
            None
        }
    }) {
        Some((true, _)) => Ok(()),
        Some((false, _)) => Err("eio"),
        None => Err("wall"),
    }
}

/// `gf100_grctx_generate` for gk104 (RESUME.md §4.1), the firmware path. `inst` is the kgr channel's instance block
/// (VRAM offset), `gold` the GOLD window (VRAM offset, mapped identity in the instance's VM), `ctx_size` method 0x10's
/// answer. `Ok(size)` = the golden image is saved, the channel's context is bound at 0x210, and `ucode::golden()`
/// reads `Some`. Every stage prints `[kgr] ctx stage=<name> rows=N`; a refusal prints `-> refused(<why>)`.
pub fn generate(bar0: usize, vw: fn(usize, u32), vr: fn(usize) -> u32, inst: usize, gold: usize, ctx_size: u32) -> Result<u32, String> {
    let mut io = Io { bar0, vw, vr, n: 0 };
    // ---- reads only -------------------------------------------------------------------------------------------------
    let t = match topology(&io) {
        Ok(t) => t,
        Err(e) => return refuse("topology", 0, format!("topology {}", e)),
    };
    serial_println!(
        "[kgr] ctx stage=topology rows=0 gpc_nr={} rop_nr={} tpc_nr={:?} tpc_total={} ppc_tpc_mask={:?} row_off={} tile={:?} sm={}",
        t.gpc_nr, t.rop_nr, &t.tpc_nr[..t.gpc_nr], t.tpc_total, &t.ppc_tpc_mask[..t.gpc_nr], t.row_off, &t.tile[..t.tpc_total.min(32) as usize], t.sm.len()
    );
    let c = census();
    if let Some(first) = c.first_short {
        serial_println!(
            "[kgr] ctx stage=lists rows={}/{} lists={} uncounted={} short={} -> refused({})",
            c.have, c.expect, c.lists, c.uncounted, c.short, first
        );
        return Err(format!("lists-short({})", first));
    }
    stage("lists", c.have);
    if !GR_INIT_PORTED {
        return refuse("presume", 0, String::from("gr-init-unported"));
    }
    let size = ctx_size as usize;
    if size == 0 || size > MAX_CTX || size & 3 != 0 {
        return refuse("window", 0, format!("ctx-size={:#x}", size));
    }
    let attrib = (0x20 * (ATTRIB_NR_MAX + ALPHA_NR_MAX) * t.tpc_total) as usize;
    if G_ATTRIB + attrib > GOLD_SPAN {
        return refuse("window", 0, format!("attrib-cb={:#x}", attrib));
    }
    // ---- 1. FE power force-on --------------------------------------------------------------------------------------
    io.wr(0x404170, 0x12);
    if ucode::poll(BUDGET_US, || if io.rd(0x404170) & 0x10 == 0 { Some(()) } else { None }).is_none() {
        return refuse("fe-power", io.n, String::from("fe-power-wall"));
    }
    // ---- 3. the FECS ctxsw reset (gf100_gr_fecs_reset); the firmware survives it (header) -------------------------
    io.wr(0x409614, 0x70);
    ucode::poll(10, || None::<()>);
    io.mask(0x409614, 0x700, 0x700);
    ucode::poll(10, || None::<()>);
    let _ = io.rd(0x409614);
    let cpuctl = io.rd(0x409100);
    if cpuctl & (1 << 4) != 0 || classify_fecs_word(cpuctl) == "POISON" {
        return refuse("fecs-reset", io.n, format!("fecs-halted-after-reset cpuctl={:08X}", cpuctl));
    }
    // ---- 4. FE power auto, 5. SCC RAM init -------------------------------------------------------------------------
    io.wr(0x404170, 0x10);
    if ucode::poll(BUDGET_US, || if io.rd(0x404170) & 0x10 == 0 { Some(()) } else { None }).is_none() {
        return refuse("fe-auto", io.n, String::from("fe-power-wall(auto)"));
    }
    io.wr(0x40802c, 1);
    stage("power", io.n);
    // ---- 6. the context allocation, zeroed; 7. the instance block's GR context pointer -----------------------------
    let ctx = gold + G_CTX;
    let img = ctx + CB_RESERVED;
    for off in (0..CB_RESERVED + size).step_by(4) {
        (io.vw)(ctx + off, 0);
    }
    for (base, len) in [(G_CHAN, size), (G_BUNDLE, BUNDLE_SIZE as usize), (G_PAGEPOOL, PAGEPOOL_SIZE), (G_ATTRIB, attrib)] {
        for off in (0..len).step_by(4) {
            (io.vw)(gold + base + off, 0);
        }
    }
    (io.vw)(inst + 0x210, (img as u32) | 4);
    (io.vw)(inst + 0x214, (img as u64 >> 32) as u32);
    stage("image", (CB_RESERVED + size) / 4);
    let done_inst = |io: &Io| {
        (io.vw)(inst + 0x210, 0);
        (io.vw)(inst + 0x214, 0);
    };
    // ---- 8. bind the instance to FECS (method 0x3), the image header ------------------------------------------------
    let arg = 0x8000_0000 | (inst >> 12) as u32;
    if let Err(e) = fecs_method(&mut io, 0x30, arg, 0x3, 0x10, 0x20) {
        done_inst(&io);
        return refuse("bind", io.n, format!("bind-{} r409800={:08X}", e, io.rd(0x409800)));
    }
    (io.vw)(img + 0x1c, 1);
    (io.vw)(img + 0x20, 0);
    (io.vw)(img + 0x28, 0);
    (io.vw)(img + 0x2c, 0);
    stage("bind", 4);
    // ---- 9. gf100_grctx_generate_main --------------------------------------------------------------------------------
    match main_seq(&mut io, &t, gold) {
        Ok(n) => stage("main", n),
        Err(e) => {
            done_inst(&io);
            return refuse("main", io.n, e);
        }
    }
    // ---- 10. wfi + golden save (method 0x9) --------------------------------------------------------------------------
    if let Err(e) = fecs_method(&mut io, 0x3, arg, 0x9, 0x1, 0x2) {
        done_inst(&io);
        return refuse("save", io.n, format!("save-{} r409800={:08X}", e, io.rd(0x409800)));
    }
    io.mask(0x409b00, 0x8000_0000, 0);
    stage("save", 1);
    // ---- 11. copy out (gr->data), 12. done_inst ----------------------------------------------------------------------
    let data: Vec<u32> = (0..size / 4).map(|i| (io.vr)(img + i * 4)).collect();
    let nz = data.iter().filter(|w| **w != 0).count();
    done_inst(&io);
    stage("copy", data.len());
    if nz == 0 {
        return refuse("copy", data.len(), String::from("image-all-zero"));
    }
    // ---- the channel's context (gf100_gr_chan_bind, RESUME.md §4.6): golden copy, header, 0x210/0x214 ------------
    let chan = gold + G_CHAN;
    for (i, w) in data.iter().enumerate() {
        (io.vw)(chan + i * 4, *w);
    }
    for (off, v) in [(0xf4usize, 0u32), (0xf8, 0), (0x10, 0), (0x14, 0), (0x18, 0), (0x1c, 1), (0x20, 0), (0x28, 0), (0x2c, 0)] {
        (io.vw)(chan + off, v);
    }
    (io.vw)(inst + 0x210, (chan as u32) | 4);
    (io.vw)(inst + 0x214, (chan as u64 >> 32) as u32);
    ucode::set_golden(chan as u64, ctx_size);
    serial_println!(
        "[kgr] ctx stage=done rows={} size={:#x} nonzero={} img={:#x} chan={:#x} ctx210={:08X} -> golden",
        io.n, size, nz, img, chan, (io.vr)(inst + 0x210)
    );
    Ok(ctx_size)
}
