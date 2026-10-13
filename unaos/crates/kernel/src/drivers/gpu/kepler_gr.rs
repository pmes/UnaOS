//! CHARTER: Kernel — driver (KEPLERGR4 rmbp-ledger B627: the GK107 GR engine's first readable class method, behind `tests kgr`)
//!
//! KEPLERGR4 — the rung after KEPLERGR3 (B503): the class GR exposes, the SET_OBJECT that names it, and ONE
//! method whose effect the CPU can read back — 256 bytes carried by GR's inline-to-memory (I2M) unit from the
//! pushbuffer page into a second pinned VRAM page. Design and citation table: docs/dev/evidence/keplergr4/
//! keplergr4.md. Declared from `kepler_fifo.rs` as `kepler_fifo::gr4` (`#[path]`, gpu/mod.rs untouched), under
//! exactly `kgr`'s gate (x86_64 + `nvidia-kepler` + `nvidia-kepler-takeover`), and called only from
//! `kgr::tests_kgr` (R80: nothing at boot). It owns no IO: `kgr` reads and writes, this file holds the facts,
//! the push image and the witness.
//!
//! UCODE GATE (the finding, keplergr4.md §3): nouveau never runs a GK104-class GR channel without FECS/GPCCS running its
//! context-switch microcode (an inference from its order, not a measured wall) (nouveau gr/gf100.c:2343-2464 `gf100_gr_init` ends in `gf100_gr_init_ctxctl`; :1832-1880
//! loads FECS/GPCCS IMEM+DMEM and starts the hub at 0x409100 = 2 (:1867-1869); a golden context is generated per channel at
//! :440-443). Flight 28 read FECS at rest `cpuctl=00000010` (HALTED) (f28-boot1.log:3260). Nouveau's ucode for
//! this chip is its own (`gk104_grhub_code`/`gk104_grgpc_code`, fuc/hubgk104.fuc3.h, MIT, built from nouveau's
//! hub.fuc/gpc.fuc), or NVIDIA's `fuc409c/409d/41ac/41ad` behind `NvGrUseFW` (gf100.c:2596-2611). Loading either
//! is Peter's ruling (R52's shape), not this rung's: with FECS halted the rung STOPS before the SET_OBJECT and
//! the verdict is `PASS (ucode-gated)`. The push below is built and pushed only when FECS reads running.
//!
//! Facts only (R95 §2): class ids, method offsets and field values, each cited file:line; no text copied.

use super::host as kf;

// ---- the class (G8' — `[kgr] class=`) ---------------------------------------------------------------------
/// The classes nouveau's GK104-family GR exposes, in its `sclass` order (gr/gk104.c:486-491; fwif load-then-nofw :496-499): FERMI_TWOD_A,
/// KEPLER_INLINE_TO_MEMORY_A, KEPLER_A, KEPLER_COMPUTE_A — ids nvif/class.h:53, :58, :190, :266. GK107 (PMC_BOOT_0
/// chipset E7) is a gk104_gr part. Fermi+ GR has no readable class-list register (KEPLERGR3, keplergr3.md): the
/// class is PINNED from the chip, and PTOP (engine type 0) confirms only that the GR engine exists.
pub const SCLASS: [u32; 4] = [0x902D, 0xA040, 0xA097, 0xA0C0];
/// The class the rung binds: KEPLER_A, whose method header NVIDIA publishes (open-gpu-doc classes/3d/cla097.h)
/// and which carries the I2M methods below; KEPLER_INLINE_TO_MEMORY_A (A040) has no public header in either
/// source read, so it is listed, not bound. Nouveau's GK104 list carries no memory-to-memory class (9039 is
/// Fermi's, class.h:56; nouveau_bo.c:1027 is its only user) — the GR-side copy on Kepler is I2M.
pub const CLASS: u32 = 0xA097;
pub const SUBCH: u32 = 0;

// ---- the methods (open-gpu-doc classes/3d/cla097.h) ---------------------------------------------------------
pub const M_SET_OBJECT: u32 = 0x0000; // cla097.h:34
pub const M_LINE_LENGTH_IN: u32 = 0x0180; // cla097.h:109
pub const M_LINE_COUNT: u32 = 0x0184; // cla097.h:112
pub const M_OFFSET_OUT_UPPER: u32 = 0x0188; // cla097.h:115 (VALUE 7:0)
pub const M_OFFSET_OUT: u32 = 0x018C; // cla097.h:118
pub const M_PITCH_OUT: u32 = 0x0190; // cla097.h:121
pub const M_LAUNCH_DMA: u32 = 0x01B0; // cla097.h:160
pub const M_LOAD_INLINE_DATA: u32 = 0x01B4; // cla097.h:193
pub const M_REPORT_SEM: u32 = 0x1B00; // cla097.h:3265-3274 (A..D)
/// LAUNCH_DMA: DST_MEMORY_LAYOUT PITCH (0:0 = 1, cla097.h:161-163) | COMPLETION_TYPE FLUSH_DISABLE (5:4 = 0, :164-165)
/// | INTERRUPT_TYPE NONE (9:8 = 0, :168-169) | SYSMEMBAR_DISABLE FALSE (6:6 = 0, :189-190).
pub const LAUNCH_DMA_PITCH: u32 = 0x0000_0001;
/// SET_REPORT_SEMAPHORE_D: RELEASE | STRUCTURE_SIZE ONE_WORD (bit 28) — KEPLERGR3's word, unchanged.
pub const REPORT_SEM_RELEASE_ONE_WORD: u32 = 0x1000_0000;

/// The readable effect: 256 bytes = 64 dwords, one line.
pub const BYTES: usize = 256;
pub const DWORDS: usize = BYTES / 4;
/// Dword i of the inline payload: "KG" | 0x40 | i — never the zero the destination page is cleared to.
#[inline]
pub const fn pattern(i: usize) -> u32 {
    0x4B47_4000 | (i as u32 & 0xFF)
}

/// SEC_OP NON_INC_METHOD (3, bits 31:29) | METHOD_COUNT 28:16 | SUBCHANNEL 15:13 | ADDRESS 11:0 (dwords) —
/// every data word goes to the SAME method (LOAD_INLINE_DATA). // open-gpu-doc cla06f.h:160-176
#[inline]
pub fn pb_hdr_non_inc(subc: u32, method: u32, count: u32) -> u32 {
    0x6000_0000 | (count << 16) | (subc << 13) | (method >> 2)
}

/// Push words for one GP entry: SET_OBJECT, the I2M line (dst = `dst`, 256 bytes, pitch), its 64 inline data
/// words, KEPLERGR3's report semaphore (`grsem` <- `gr_payload`), and the host WFI semaphore release
/// (`fence` <- `fence_payload`) that lands only after GR went idle on everything before it. Returns the count.
pub const PUSH_MAX: usize = 96;
pub fn build_push(dst: usize, grsem: usize, gr_payload: u32, fence: usize, fence_payload: u32, out: &mut [u32; PUSH_MAX]) -> usize {
    let mut n = 0usize;
    let mut put = |v: u32| {
        if n < PUSH_MAX {
            out[n] = v;
            n += 1;
        }
    };
    put(kf::pb_hdr(SUBCH, M_SET_OBJECT, 1));
    put(CLASS);
    put(kf::pb_hdr(SUBCH, M_LINE_LENGTH_IN, 5)); // LINE_LENGTH_IN .. PITCH_OUT, incrementing
    put(BYTES as u32);
    put(1);
    put(((dst >> 32) as u32) & 0xFF);
    put(dst as u32);
    put(BYTES as u32);
    put(kf::pb_hdr(SUBCH, M_LAUNCH_DMA, 1));
    put(LAUNCH_DMA_PITCH);
    put(pb_hdr_non_inc(SUBCH, M_LOAD_INLINE_DATA, DWORDS as u32));
    for i in 0..DWORDS {
        put(pattern(i));
    }
    put(kf::pb_hdr(SUBCH, M_REPORT_SEM, 4));
    put((grsem >> 32) as u32);
    put(grsem as u32);
    put(gr_payload);
    put(REPORT_SEM_RELEASE_ONE_WORD);
    put(kf::pb_hdr(SUBCH, kf::M_HOST_SEM, 4));
    put((fence >> 32) as u32);
    put(fence as u32);
    put(fence_payload);
    put(kf::SEM_RELEASE_WFI);
    n
}

/// `[kgr] class=` — the pinned class, the list it was pinned from, and what PTOP said about GR.
pub fn class_line(chip: u32, rl: u32, eng: u32) {
    let o = |v: u32| if v == kf::NONE { alloc::string::String::from("-") } else { alloc::format!("{}", v) };
    serial_println!(
        "[kgr] class=0x{:04X} chipset={:02X} sclass={:04X},{:04X},{:04X},{:04X} ptop_gr=rl{}/eng{} src={}",
        CLASS, chip, SCLASS[0], SCLASS[1], SCLASS[2], SCLASS[3], o(rl), o(eng),
        if chip == 0xE7 { "pinned(gk104_gr sclass; no class register)" } else { "unpinned" }
    );
}

/// The KGR4 witness, one line after `:: KGR:`.
pub fn witness(class: &str, set_object: &str, bytes: usize, landed: &str, preamble: &str, fecs: &str, verdict: &str) {
    serial_println!(
        ":: KGR4: class={} set_object={} method=i2m bytes={} landed={} preamble={} fecs={} -> {} ::",
        class, set_object, bytes, landed, preamble, fecs, verdict
    );
}
