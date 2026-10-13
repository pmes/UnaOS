//! CHARTER: Kernel — driver
//! GEN7R11 (B654, R121) — rung R11: THE FIRST COMPOSITED SURFACE on the Ivy Bridge RCS — a window's pixels drawn as
//! two textured triangles through the sampler into a window-sized render target, read back and compared
//! pixel-for-pixel with the CPU composite of the same window.
//!
//! Design and the row table: `docs/dev/evidence/gen7r11/GEN7R11.md`. A child of `gen7_r10.rs` (declared by `#[path]`
//! at its tail, as `mod quad`), run by R10's `run` right after R10's own witness line, under `tests gen7` only (R80:
//! nothing here runs at boot). Gated on R10 `r10-triangle-verified` THIS run. `use super::*` sees R10's private
//! encodings (gfx3d headers, SS_DW0, the EU builders, `Ring`, `poke`, `KSP0`, the state-page offsets) and, through
//! R10's own glob, R9's and gen7's — the mechanism R10 compiles on.
//!
//! THE RUNG: R10's stream with the rows GEN7R10.md §6 left open, filled: a 3-element vertex (header, position,
//! TEXCOORD), SBE with one attribute, a PS with one sampler and a two-entry binding table (RT, source), a
//! SAMPLER_STATE (nearest, clamp, non-normalized coordinates = texel units), and the 8-instruction sampling kernel
//! (2 pln, sampler send, 4 mov, RT-write EOT). The source is the front window's surface, copied 1:1 by the
//! compositor's own nearest sampler (`video::wm::thumb`, read-only) into R11's pages; the target is the window's
//! composited size (w*scale x h*scale, the compositor's integer nearest upscale). The CPU reference is that upscale:
//! `want(x, y) = src[y / scale][x / scale]`. The GPU does NOT present (R12 is the present): the proof is the read-back.
//!
//! SOURCES (R83 / R95 §2): `rs` = i915 gt/gen7_renderstate.c (linux 24a19b95, SPDX MIT, sha256 52e5ca5b…; the null
//! state batch rs:18-258 is a textured-quad pipeline and pairs every open row literally), `rc` = gt/gen7_renderclear.c.
//! The PRM page cites (`v1p1`/`v2p1`/`v4p1`/`v4p3`, GEN7R10.md §1) are CARRIED from GEN7R10.md §6: the IVB PRM was not
//! reachable from the cloud seat that wrote this rung, and the bench re-reads them at the fold. Values only; no text
//! is copied. A row whose `src` is `None` DECLINES the rung by name and nothing is written.

use super::*;

/// R11's rows: the ones that differ from R10's 37 (which R11 re-runs unchanged), in command-stream order.
const R11_PAGES: [Page; 17] = [
    Page { name: "3DSTATE_URB_VS", field: "urb_vue48", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:34-35 rs:38-39") },
    Page { name: "3DSTATE_SBE", field: "sbe_attr1", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:263-265 rs:111-112") },
    Page { name: "3DSTATE_WM", field: "wm_bary", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:276-282 rs:89-90") },
    Page { name: "BLEND_STATE", field: "blend_off", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:366-372 rs:163-164") },
    Page { name: "3DSTATE_SAMPLER_STATE_POINTERS_PS", field: "samp_ptr", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:30 rs:109-110") },
    Page { name: "SAMPLER_STATE", field: "sampler", owed: "IVB-PRM-Vol4-Pt1", src: Some("v4p1:91-100 rs:179-182") },
    Page { name: "BINDING_TABLE", field: "bt2", owed: "IVB-PRM-Vol4-Pt1", src: Some("v4p1:61 rs:235-236") },
    Page { name: "SURFACE_STATE-source", field: "src_ss", owed: "IVB-PRM-Vol4-Pt1", src: Some("v4p1:62-78 rc:151-168") },
    Page { name: "SURFACE_STATE-rt", field: "rt_ss", owed: "IVB-PRM-Vol4-Pt1", src: Some("v4p1:62-81 rc:151-168") },
    Page { name: "3DSTATE_PS", field: "ps_samp", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:283-290 rs:125-132") },
    Page { name: "EU-pln", field: "pln", owed: "IVB-PRM-Vol4-Pt3", src: Some("v4p3:131,203 rs:195-202") },
    Page { name: "EU-sampler-send", field: "sample", owed: "IVB-PRM-Vol4-Pt1", src: Some("v4p1:123-135 v1p1:18 rs:203-206") },
    Page { name: "EU-mov-rt-write", field: "rtw", owed: "IVB-PRM-Vol4-Pt3", src: Some("v4p3:127,129,215-218 v4p1:225,229,280-286 rs:207-226") },
    Page { name: "3DSTATE_DRAWING_RECTANGLE", field: "rect_wh", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:244-246 rs:147-150") },
    Page { name: "3DSTATE_VERTEX_BUFFERS", field: "vb16", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:77-80 rs:140-144") },
    Page { name: "3DSTATE_VERTEX_ELEMENTS", field: "ve3", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:84-88 v4p1:79 rs:133-139") },
    Page { name: "3DPRIMITIVE", field: "prim6", owed: "IVB-PRM-Vol2-Pt1", src: Some("v2p1:75,90-94 rs:151-157") },
];

// ---- The rows R11 changes (GEN7R11.md §2 names every bit; every bit not named is R10's value) -------------------
/// 3DSTATE_SAMPLER_STATE_POINTERS_PS: opcode 0, sub 2Fh, 2 dwords (v2p1:30); exactly rs:109 `0x782f0000`.
const SAMPLER_PTRS_PS: u32 = gfx3d(0, 0x2F, 2);
/// 3DSTATE_VERTEX_ELEMENTS with 3 elements: 1 + 2*3 = 7 dwords (v2p1:84); exactly rs:133 `0x78090005`.
const VERTEX_ELEMENTS_3: u32 = gfx3d(0, 0x09, 7);
/// VERTEX_ELEMENT_STATE 2: VB 0, Valid (25), R32G32_FLOAT (085h at 24:16, v4p1:79), source offset 8 (11:0); R10's
/// component word STORE_SRC, STORE_SRC, STORE_0, STORE_1_FP -> VUE dwords 8-11 = (u, v, 0.0, 1.0) (v2p1:85-88).
/// rs:138-139 is the same element at offset 4 with R16G16_SSCALED (`0x02f60004 0x11230000`).
const VE2: [u32; 2] = [(1 << 25) | (0x085 << 16) | 8, VE1[1]];
/// VERTEX_BUFFER_STATE DW0: R10's, pitch 16 bytes (x, y, u, v as R32 floats) (v2p1:78-79).
const VB16_DW0: u32 = (1 << 14) | 16;
/// SBE DW1: 1 SF output attribute (27:22), URB read length 1 (15:11), read offset 1 (9:4) = VUE dword 8, the
/// texcoord (v2p1:264-265); exactly rs:112 `0x00400810`. URB_VS entry size 1 (2 x 64 B rows) holds the 48 B VUE.
const SBE1_DW1: u32 = (1 << 22) | (1 << 11) | (1 << 4);
/// PS DW2: Sampler Count 1 (29:27), Binding Table Entry Count 2 (25:18) (v2p1:284); exactly rs:127 `0x08080000`.
const PS2_DW2: u32 = (1 << 27) | (2 << 18);
/// PS DW4: R10's max threads 0x2F and SIMD16 dispatch, plus Attribute Enable (bit 10, v2p1:286): SBE now has an
/// attribute. rs:129 `0x28000402` carries the same bits 10 and 1 (with max threads 0x28).
const PS2_DW4: u32 = (0x2F << 24) | (1 << 10) | (1 << 1);
/// SAMPLER_STATE (4 dwords, v4p1:91-100): DW0 = 0: sampler enabled, mip filter NONE, mag/min MAPFILTER_NEAREST;
/// DW1 = 0: LODs 0; DW2 = 0: border colour pointer (unused under CLAMP); DW3: Non-normalized Coordinate Enable
/// (bit 10: u, v in texels), TCX/TCY/TCZ = TEXCOORDMODE_CLAMP (2h at 8:6, 5:3, 2:0). Exactly rs:179-182.
const SAMPLER: [u32; 4] = [0, 0, 0, (1 << 10) | (2 << 6) | (2 << 3) | 2];
const _: () = assert!(
    SAMPLER_PTRS_PS == 0x782F_0000 && VERTEX_ELEMENTS_3 == 0x7809_0005 && VE2[0] == 0x0285_0008 && VE2[1] == 0x1123_0000
        && SBE1_DW1 == 0x0040_0810 && PS2_DW2 == 0x0808_0000 && (PS2_DW4 & 0x00FF_FFFF) == 0x0000_0402
        && SAMPLER[3] == 0x0000_0492 && VB16_DW0 == 0x0000_4010
);

// ---- EU kernel: interpolate (u, v), sample, write (v4p3 formats; GEN7R11.md §3 decodes every field) -------------
const EU_PLN: u32 = 0x5A; // v4p3:131
const SFID_SAMPLER: u32 = 0x2; // v1p1:18 "Sampler"
/// A `<8;8,1>` source region on GRF `reg`, subregister 0 (RegNum at 12:5 of the operand dword, HorzStride 1,
/// Width 8, VertStride 8; v4p3:96,103) — R10's SEND_DW2 shape, for any register.
const fn rgn8(reg: u32) -> u32 {
    (reg << 5) | (1 << 16) | (3 << 18) | (4 << 21)
}
/// A send's DW1: dst RegFile `file` (1:0), DstType UW (4:2), Src0 GRF:F, Src1 IMM:D, Dst.RegNum (28:21),
/// HorzStride 1 (30:29). `(ARF, 0)` is R10's SEND_DW1 (rs:224); `(GRF, 12)` is the sampler send's (rs:204).
const fn send_dw1(file: u32, reg: u32) -> u32 {
    file | (2 << 2) | (1 << 5) | (7 << 7) | (3 << 10) | (1 << 12) | (reg << 21) | (1 << 29)
}
/// The sampler's writeback: R, G, B, A for slots 15:0 at g12..g19 (two GRFs per channel).
const RESP: u32 = 12;
/// The sample message descriptor (v4p1:123-135): Message Length 4 (28:25: u, v x 2 GRF), Response Length 8 (24:20),
/// no header (19), SIMD Mode SIMD16 (18:17 = 2), Message Type 0 = sample (16:12), Sampler Index 0 (11:8), Binding
/// Table Index 1 (7:0) = the source SURFACE_STATE. Exactly rs:206 `0x08840001`.
const SAMPLE_DESC: u32 = (4 << 25) | (8 << 20) | (2 << 17) | 1;
/// The kernel: `pln(16) g113 g6.0 g2` (u), `pln(16) g115 g6.4 g2` (v), `send(16) g12 g113 sampler`, four
/// `mov(16) g113+2i g12+2i`, R10's EOT render-target write. 32 dwords at KSP0 = 0x40 (ends 0xC0; the 128-byte
/// prefetch stays in the page). The payload is R10's: g0-g1 header, g2-g5 perspective-pixel barycentrics, g6 the
/// attribute's setup constants (Dispatch GRF Start 6): dwords 0-3 for u, 4-7 for v.
const fn kernel11() -> [u32; 32] {
    let mut k = [0u32; 32];
    let mut i = 0;
    while i < 2 {
        k[i * 4] = eu_dw0(EU_PLN, 0);
        // Dst g113/g115:F, Src0 GRF:F (the setup scalar <0;1,0>), Src1 GRF:F (bit 10 = Src1.RegFile GRF).
        k[i * 4 + 1] = mov_dw1(PAY + 2 * i as u32, FILE_GRF, TYPE_F) | (FILE_GRF << 10);
        k[i * 4 + 2] = (6 << 5) | (16 * i as u32); // g6.0 (u) / g6.4 (v): RegNum 6, SubReg byte 0 / 16
        k[i * 4 + 3] = rgn8(2); // the barycentrics g2 <8;8,1>
        i += 1;
    }
    k[8] = eu_dw0(EU_SEND, SFID_SAMPLER);
    k[9] = send_dw1(FILE_GRF, RESP);
    k[10] = rgn8(PAY);
    k[11] = SAMPLE_DESC;
    let mut c = 0;
    while c < 4 {
        k[12 + c * 4] = eu_dw0(EU_MOV, 0);
        k[12 + c * 4 + 1] = mov_dw1(PAY + 2 * c as u32, FILE_GRF, 0);
        k[12 + c * 4 + 2] = rgn8(RESP + 2 * c as u32);
        k[12 + c * 4 + 3] = 0;
        c += 1;
    }
    k[28] = eu_dw0(EU_SEND, SFID_DP_RC);
    k[29] = SEND_DW1;
    k[30] = SEND_DW2;
    k[31] = SEND_DESC;
    k
}
const KERNEL11: [u32; 32] = kernel11();
/// rs:195-226, the null state's kernel, literally: the builder above must reproduce it dword for dword.
const RS_KERNEL: [u32; 32] = [
    0x0080_005a, 0x2e20_77bd, 0x0000_00c0, 0x008d_0040, 0x0080_005a, 0x2e60_77bd, 0x0000_00d0, 0x008d_0040,
    0x0280_0031, 0x2180_1fa9, 0x008d_0e20, 0x0884_0001, 0x0080_0001, 0x2e20_03bd, 0x008d_0180, 0x0000_0000,
    0x0080_0001, 0x2e60_03bd, 0x008d_01c0, 0x0000_0000, 0x0080_0001, 0x2ea0_03bd, 0x008d_0200, 0x0000_0000,
    0x0080_0001, 0x2ee0_03bd, 0x008d_0240, 0x0000_0000, 0x0580_0031, 0x2000_1fa8, 0x008d_0e20, 0x9003_1000,
];
const fn same(a: &[u32; 32], b: &[u32; 32]) -> bool {
    let mut i = 0;
    while i < 32 {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}
const _: () = assert!(same(&KERNEL11, &RS_KERNEL) && send_dw1(0, 0) == SEND_DW1 && rgn8(PAY) == SEND_DW2);

// ---- The window and the pages -----------------------------------------------------------------------------------
/// R11's own GGTT window: ring, state, kernel, then the source pages, then the render-target pages. GGTT 0x13420000:
/// RING_START bits[31:29] zero, past R10's 0x13410..0x13413 with a gap; every slot and both neighbours proven unowned
/// at run time before any write, or the rung refuses. The window ends below R5's first far probe (0x20000).
const R11_BASE_SLOT: usize = 0x13420;
const R11_SLOT_END: usize = 0x20000;
const PG_SRC0: usize = 3;
/// The state page's R11 additions (R10's offsets otherwise): SAMPLER_STATE (32-byte aligned, v2p1:30) and the
/// source SURFACE_STATE (32-byte aligned, v4p1:61). The 6-vertex buffer at R10's OFF_VB is 96 bytes (0x300..0x360).
const OFF_SAMP: u32 = 0x240;
const OFF_SS2: u32 = 0x260;
/// The fence the final post-sync writes, and the seed the CPU leaves there first.
const R11_FENCE: u32 = 0x3D11_C0DE;
const R11_FENCE_SEED: u32 = 0x5EED_3D11;
/// The RT seed: alpha 0x10, which no sampled pixel carries (the source copy's alpha is 0), so `pixels` = dwords
/// that differ from the seed is a sound count of what the GPU wrote.
const R11_SEED: u32 = 0x1010_1010;
/// Refusal caps, before any write: the render target's bytes and either dimension.
const RT_MAX_BYTES: usize = 24 << 20;
const DIM_MAX: usize = 8192;

/// The pages, allocated once per size and held for the boot (GEN7TLB, gen7.md §2.6): (base VA, page count). A run
/// needing more pages allocates afresh and the smaller block stays held, never freed.
static HELD: crate::sync::Mutex<(usize, usize)> = crate::sync::Mutex::new((0, 0));

/// The draw's geometry: the source window and the render target.
#[derive(Clone, Copy)]
struct Geom {
    id: u32,
    w: usize,
    h: usize,
    scale: usize,
    src_pitch: usize,
    rt_w: usize,
    rt_h: usize,
    rt_pitch: usize,
}

impl Geom {
    fn src_pages(&self) -> usize {
        (self.src_pitch * self.h).div_ceil(4096)
    }
    fn rt_pages(&self) -> usize {
        (self.rt_pitch * self.rt_h).div_ceil(4096)
    }
    fn slots(&self) -> usize {
        PG_SRC0 + self.src_pages() + self.rt_pages()
    }
}

/// What the RCS run did. Every field a reading.
#[derive(Clone, Copy, Default)]
struct Out11 {
    cand: &'static str,
    landed: bool,
    armed: bool,
    hit: bool,
    read: u32,
    head: u32,
    tail: u32,
    idle: bool,
    disabled: bool,
    regs_restored: bool,
    fw_restored: bool,
    ptes_restored: bool,
    cyc: u64,
    pixels: u32,
    mismatch: u32,
    first: Option<(u32, u32, u32, u32)>,
    passed: &'static str,
}

/// R11 under `tests gen7`, after R10's line: gated on R10's verified triangle this run, then the table, the window,
/// the draw, the compare. One `[gen7] r11` line and one `:: GEN7: r11 … ::` line on every path.
pub(super) fn run(r10: &str) {
    let cited = R11_PAGES.iter().filter(|p| p.src.is_some()).count();
    let rows = R11_PAGES.len();
    if r10 != "r10-triangle-verified" {
        serial_println!(
            "[gen7] r11 draw=quad r10={} rows_cited={}/{} writes=0 -> GATED reason=r10-not-verified",
            r10, cited, rows
        );
        serial_println!(":: GEN7: r11 draw=quad r10={} -> GATED ::", r10);
        return;
    }
    if let Some(p) = R11_PAGES.iter().find(|p| p.src.is_none()) {
        serial_println!(
            "[gen7] r11 draw=quad r10={} rows_cited={}/{} writes=0 -> DECLINED reason=page-{}-uncited field={} owed={}",
            r10, cited, rows, p.name, p.field, p.owed
        );
        serial_println!(":: GEN7: r11 draw=quad -> DECLINED ::");
        return;
    }
    let refuse = |why: &str| {
        serial_println!("[gen7] r11 draw=quad r10={} rows_cited={}/{} writes=0 -> REFUSED reason={}", r10, cited, rows, why);
        serial_println!(":: GEN7: r11 draw=quad -> REFUSED reason={} ::", why);
    };

    // ---- the source: the front window (highest z, ties by id), read-only through wm's public snapshot ---------------
    let mut best: Option<crate::video::wm::WindowInfo> = None;
    for id in 1..=crate::video::wm::slots() as u32 {
        if let Some(i) = crate::video::wm::info(id) {
            if i.w > 0 && i.h > 0 && best.map_or(true, |b| (i.z, i.id) > (b.z, b.id)) {
                best = Some(i);
            }
        }
    }
    let Some(win) = best else { return refuse("no-window") };
    let scale = win.scale.max(1);
    let (rt_w, rt_h) = (win.w.saturating_mul(scale), win.h.saturating_mul(scale));
    let rt_pitch = (rt_w.saturating_mul(4) + 63) & !63;
    if rt_w > DIM_MAX || rt_h > DIM_MAX || rt_pitch.saturating_mul(rt_h) > RT_MAX_BYTES {
        return refuse("rt-too-large");
    }
    let g = Geom { id: win.id, w: win.w, h: win.h, scale, src_pitch: (win.w * 4 + 63) & !63, rt_w, rt_h, rt_pitch };
    // The copy: `thumb` at tw = w, th = h is the compositor's own nearest sampler at 1:1 (aspect fit degenerates to the
    // full frame), alpha masked to 0. The window's backing store is video/'s; R12 (present) is where that moves.
    let mut src: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
    if src.try_reserve_exact(g.w * g.h).is_err() {
        return refuse("alloc");
    }
    src.resize(g.w * g.h, 0);
    if !crate::video::wm::thumb(g.id, &mut src, g.w, g.h) {
        return refuse("no-surface");
    }

    let bank = *super::super::super::ladder::BANK.lock();
    let Some(b) = bank else { return refuse("no-boot-bank") };
    // SAFETY: `bar0` is the IGD BAR0 mapping `igpu::init` published and banked; never unmapped. `exec11` writes only
    // its own proven-unowned GGTT window, `0x101008`, one forcewake request (restored) and the four RCS ring registers
    // (restored), and refuses before any write on every doubt it can read.
    let t0 = crate::arch::now_cycles();
    let res = unsafe { exec11(b.bar0, b.bar0_size, &g, &src) };
    let us = cycles_to_us(crate::arch::now_cycles().saturating_sub(t0));
    let o = match res {
        Err(why) => return refuse(why),
        Ok(o) => o,
    };
    let total = (g.rt_w * g.rt_h) as u32;
    //  r11-quad-verified    THE WIN: the fence landed, the head retired the stream, every RT pixel was written and
    //                       equals the CPU composite (the compositor's integer nearest upscale of the same window).
    //  r11-drew-nothing     the fence landed with the RT untouched: suspect the sampler/attribute path's dispatch.
    //  r11-quad-mismatch    the GPU wrote, and some pixel differs from the CPU composite (`first=` names it).
    //  r11-fence-miss / r11-head-stuck passed=<cmd> / R6's envelope verdicts as r11-*.
    let verdict = if !o.landed {
        "r11-claim-write-void"
    } else if !o.armed {
        "r11-enable-void"
    } else if !o.disabled {
        "r11-ring-would-not-disable"
    } else if o.hit && o.idle && o.pixels == total && o.mismatch == 0 {
        "r11-quad-verified"
    } else if o.hit && o.pixels == 0 {
        "r11-drew-nothing"
    } else if o.hit {
        "r11-quad-mismatch"
    } else if o.head == o.tail {
        "r11-fence-miss"
    } else {
        "r11-head-stuck"
    };
    let sampled = if o.pixels > 0 && o.mismatch == 0 { "ok" } else { "no" };
    let (fx, fy, got, want) = o.first.unwrap_or((0, 0, 0, 0));
    serial_println!(
        "[gen7] r11 draw=quad src={}:{}x{}@s{} rt={}x{}:b8g8r8a8 sampled={} mismatch={} landed={} r10={} rows_cited={}/{} ring=rcs cand={} prim=trilist verts=6 pitch={} slots={} pixels={}/{} first={} at=({},{}) got=0x{:08X} want=0x{:08X} fence=0x{:08X} read=0x{:08X} head={:#x} tail={:#x} passed={} idle={} restored=regs:{},fw:{},ptes:{} gpu_us={} us={} -> {}",
        g.id,
        g.w,
        g.h,
        g.scale,
        g.rt_w,
        g.rt_h,
        sampled,
        o.mismatch,
        o.hit as u32,
        r10,
        cited,
        rows,
        o.cand,
        g.rt_pitch,
        g.slots(),
        o.pixels,
        total,
        if o.first.is_some() { "mismatch" } else { "none" },
        fx,
        fy,
        got,
        want,
        R11_FENCE,
        o.read,
        o.head,
        o.tail,
        o.passed,
        o.idle as u32,
        o.regs_restored as u32,
        o.fw_restored as u32,
        o.ptes_restored as u32,
        cycles_to_us(o.cyc),
        us,
        verdict
    );
    serial_println!(
        ":: GEN7: r11 draw=quad src={} rt={}x{} sampled={} mismatch={} landed={} -> {} ::",
        g.id,
        g.rt_w,
        g.rt_h,
        sampled,
        o.mismatch,
        o.hit as u32,
        if verdict == "r11-quad-verified" { "PASS" } else { verdict }
    );
}

/// The RCS run. `Err(why)` before any write; `Ok(out)` once the window was claimed.
///
/// # Safety
/// `bar0`/`bar0_size` are the IGD BAR0 mapping the ladder banked. Writes: the window's PTEs (restored), `0x101008`
/// (write-any-value, gen7.md §2.6), one forcewake request register (restored by `fw_release`), the four RCS ring
/// registers (restored to their entry images, only behind a disabled ring). No display register.
unsafe fn exec11(bar0: usize, bar0_size: usize, g: &Geom, src: &[u32]) -> Result<Out11, &'static str> {
    let n = g.slots();
    let pg_rt = PG_SRC0 + g.src_pages();
    let flsh = g7regs::HYP_GFX_FLSH_CNTL;
    let need = (GTT_BASE + (R11_BASE_SLOT + n + 1) * 4).max(flsh + 4).max(g7regs::IVB_FORCEWAKE_MT_ACK + 4);
    if R11_BASE_SLOT + n + 1 >= R11_SLOT_END || R11_BASE_SLOT + n + 1 >= GGTT_SLOTS || bar0_size < need {
        return Err("window-too-large");
    }
    if src.len() < g.w * g.h {
        return Err("src-short");
    }
    let Some(&(cand, class_pin, req_off, ack_off, ack_mask, srcp, mask_form)) = R6_CANDS.iter().find(|c| c.0 == "mt") else {
        return Err("no-mt-row");
    };

    // ---- census: every window slot and both neighbours, before any write (R10's legs over n slots) -------------------
    let slot = |k: usize| GTT_BASE + (R11_BASE_SLOT + k) * 4;
    let prev_off = GTT_BASE + (R11_BASE_SLOT - 1) * 4;
    let next_off = slot(n);
    let fill = rd(bar0, slot(0));
    let (prev, next) = (rd(bar0, prev_off), rd(bar0, next_off));
    if prev != fill || next != fill || (0..n).any(|k| rd(bar0, slot(k)) != fill) {
        return Err("window-owned");
    }
    if fill != 0 {
        let bdsm = crate::arch::pci::read_config_32(0, 0, 0, 0xB0) & 0xFFF0_0000;
        let wellformed = (fill & 1) != 0 && (fill & 0xFFFF_F000) != 0 && (fill & 0xF0) == 0;
        let is_bdsm = (fill >> 12) == (bdsm >> 12);
        let far = R5_FAR_PROBE.iter().all(|&s| GTT_BASE + s * 4 + 4 <= bar0_size && rd(bar0, GTT_BASE + s * 4) == fill);
        if !(wellformed && is_bdsm && far) {
            return Err("fill-not-scratch");
        }
    }
    let base_img = fill;

    // ---- the RCS must be off: a live ring is never armed under ------------------------------------------------------
    let start_pre = rd(bar0, g7regs::RCS_RING_START);
    let ctl_pre = rd(bar0, g7regs::RCS_RING_CTL);
    let head_pre = rd(bar0, g7regs::RCS_RING_HEAD);
    let tail_pre = rd(bar0, g7regs::RCS_RING_TAIL);
    if ctl_pre & 1 != 0 {
        return Err("rcs-ring-live");
    }

    // ---- the pages, held for the boot ---------------------------------------------------------------------------------
    let base_va = {
        let mut held = HELD.lock();
        if held.1 < n {
            let p = alloc_zeroed(Layout::from_size_align(n * 4096, 4096).map_err(|_| "layout")?);
            if p.is_null() {
                return Err("alloc");
            }
            *held = (p as usize, n); // a smaller earlier block stays held (GEN7TLB), never freed
        }
        held.0
    };
    let va = |k: usize| base_va + k * 4096;
    let mut ptes: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
    if ptes.try_reserve_exact(n).is_err() {
        return Err("alloc");
    }
    for k in 0..n {
        let Some(pa) = crate::arch::memory::translate(va(k) as u64) else { return Err("va-unmapped") };
        if pa >= 0x1_0000_0000 {
            return Err("phys-above-4g"); // the 32-bit PTE form R4..R10 flew
        }
        let pte = (pa as u32 & 0xFFFF_F000) | GGTT_PTE_VALID;
        if pte == base_img {
            return Err("pte-indistinct");
        }
        ptes.push(pte);
    }
    let gtt = |k: usize| ((R11_BASE_SLOT + k) * 4096) as u32;
    let (state, kern, srcg, rtg) = (gtt(PG_STATE), gtt(PG_KERN), gtt(PG_SRC0), gtt(pg_rt));

    // ---- the state page (GEN7R11.md §4): R10's, plus the sampler, the second SURFACE_STATE, two BT entries ----------
    for i in 0..4096 / 4 {
        core::ptr::write_volatile((va(PG_STATE) + i * 4) as *mut u32, 0);
    }
    poke(va(PG_STATE), OFF_BLEND, &[(1 << 5) | 0x11, 0x3]); // blend off, every channel written (R10's, rs:163-164)
    poke(va(PG_STATE), OFF_CCVP, &[0x0000_0000, 0x3F80_0000]);
    // BINDING_TABLE: entry 0 -> the RT SURFACE_STATE (the RT write's index 0), entry 1 -> the source (the sample
    // message's index 1) — rs:235-236's two entries (v4p1:61).
    poke(va(PG_STATE), OFF_BT, &[OFF_SS, OFF_SS2]);
    poke(va(PG_STATE), OFF_SS, &[SS_DW0, rtg, (((g.rt_h - 1) as u32) << 16) | (g.rt_w - 1) as u32, (g.rt_pitch - 1) as u32, 0, 0, 0, 0]);
    poke(va(PG_STATE), OFF_SS2, &[SS_DW0, srcg, (((g.h - 1) as u32) << 16) | (g.w - 1) as u32, (g.src_pitch - 1) as u32, 0, 0, 0, 0]);
    poke(va(PG_STATE), OFF_SAMP, &SAMPLER);
    // The vertex buffer: two TRILIST triangles covering the RT, (x, y, u, v) as R32 floats, u/v in source texels
    // (non-normalized): (0,0)(W,0)(0,H) and (W,0)(W,H)(0,H). One affine texcoord plane, so the shared diagonal's
    // colour is the same whichever triangle owns it.
    let (fw, fh, sw, sh) = ((g.rt_w as f32).to_bits(), (g.rt_h as f32).to_bits(), (g.w as f32).to_bits(), (g.h as f32).to_bits());
    poke(
        va(PG_STATE),
        OFF_VB,
        &[0, 0, 0, 0, fw, 0, sw, 0, 0, fh, 0, sh, fw, 0, sw, 0, fw, fh, sw, sh, 0, fh, 0, sh],
    );
    poke(va(PG_STATE), OFF_FENCE, &[R11_FENCE_SEED, 0]);
    // ---- the kernel page, the source pages, the render target -------------------------------------------------------
    for i in 0..4096 / 4 {
        core::ptr::write_volatile((va(PG_KERN) + i * 4) as *mut u32, 0);
    }
    poke(va(PG_KERN), KSP0, &KERNEL11);
    for i in 0..g.src_pages() * 1024 {
        core::ptr::write_volatile((va(PG_SRC0) + i * 4) as *mut u32, 0);
    }
    for y in 0..g.h {
        for x in 0..g.w {
            core::ptr::write_volatile((va(PG_SRC0) + y * g.src_pitch + x * 4) as *mut u32, src[y * g.w + x]);
        }
    }
    for i in 0..g.rt_pages() * 1024 {
        core::ptr::write_volatile((va(pg_rt) + i * 4) as *mut u32, R11_SEED);
    }
    let vb = state + OFF_VB;

    // ---- the ring: R10's stream (GEN7R10.md §2) with R11's rows (GEN7R11.md §2) ------------------------------------
    let mut r = Ring { p: va(PG_RING) as *mut u32, d: 0, marks: [(0, ""); MARKS], n: 0 };
    for i in 0..1024 {
        core::ptr::write_volatile(r.p.add(i), MI_NOOP);
    }
    r.invalidate("invalidate-0");
    r.cmd("pipeline_select", &[PIPELINE_SELECT_3D, MI_NOOP]);
    r.invalidate("invalidate-1");
    r.cmd("flush", &[PC4, PC_FLUSH_FLAGS, 0, 0]);
    let m = BASE_ADDRESS_MODIFY;
    r.cmd("state_base_address", &[SBA_DW0, m, state | m, state | m, m, kern | m, m, m, m, m]);
    r.invalidate("invalidate-2");
    r.cmd("multisample", &[MULTISAMPLE, 0, 0, 0]);
    r.cmd("sample_mask", &[SAMPLE_MASK, 1]);
    r.cmd("pc-wa-vs", &[PC4, PC_WA_FLAGS, state + OFF_WA, 0]);
    r.cmd("pcalloc_vs", &[PCALLOC_VS, 0]);
    r.cmd("pcalloc_ps", &[PCALLOC_PS, PCALLOC_PS_DW1]);
    r.cmd("pc-cs-stall", &[PC4, PC_CS_STALL_FLAGS, 0, 0]);
    r.cmd("urb", &[URB_VS, URB_VS_DW1, URB_HS, URB_HS_DS_DW1, URB_DS, URB_HS_DS_DW1, URB_GS, URB_GS_DW1]);
    r.cmd("constant_vs", &[CONSTANT_VS, 0, 0, 0, 0, 0, 0]);
    r.cmd("vs", &[VS, 0, 0, 0, 0, 0]);
    r.cmd("hs", &[HS, 0, 0, 0, 0, 0, 0]);
    r.cmd("te", &[TE, 0, 0, 0]);
    r.cmd("ds", &[DS, 0, 0, 0, 0, 0]);
    r.cmd("gs", &[GS, 0, 0, 0, 0, 0, 0]);
    r.cmd("streamout", &[STREAMOUT, 0, 0]);
    r.cmd("clip", &[CLIP, CLIP_DW1, 0, 0]);
    r.cmd("viewport_ptrs", &[VP_SF_CLIP, OFF_SFCLIP, VP_CC, OFF_CCVP]);
    r.cmd("sampler_ptrs_ps", &[SAMPLER_PTRS_PS, OFF_SAMP]);
    r.cmd("sf", &[SF, SF_DW1, SF_DW2, 0, 0, 0, 0]);
    r.cmd("sbe", &[SBE, SBE1_DW1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    r.cmd("wm", &[WM, WM_DW1, 0]);
    r.cmd("cc_ptrs", &[BLEND_PTR, OFF_BLEND, CC_PTR, OFF_CC, DSS_PTR, OFF_DSS]);
    r.cmd("binding_table_ptr_ps", &[BT_PS, OFF_BT]);
    r.cmd("constant_ps", &[CONSTANT_PS, 0, 0, 0, 0, 0, 0]);
    r.cmd("ps", &[PS, KSP0, PS2_DW2, 0, PS2_DW4, PS_DW5, 0, 0]);
    r.cmd("pc-wa-wm", &[PC4, PC_DEPTH_STALL, 0, 0]);
    r.cmd("pc-wa-depth", &[PC4, PC_WA_FLAGS, state + OFF_WA, 0]);
    r.cmd("pc-depth-flush", &[PC4, PC_DEPTH_CACHE_FLUSH, 0, 0]);
    r.cmd("pc-depth-stall", &[PC4, PC_DEPTH_STALL, 0, 0]);
    r.cmd("depth_buffer", &[DEPTH_BUFFER, DEPTH_DW1, 0, 0, 0, 0, 0]);
    r.cmd("hier_stencil_clear", &[HIER_DEPTH_BUFFER, 0, 0, STENCIL_BUFFER, 0, 0, CLEAR_PARAMS, 0, 0]);
    r.cmd("drawing_rectangle", &[DRAWING_RECT, 0, (((g.rt_h - 1) as u32) << 16) | (g.rt_w - 1) as u32, 0]);
    r.cmd("vertex_buffers", &[VERTEX_BUFFERS_1, VB16_DW0, vb, vb + 6 * 16 - 1, 0]);
    r.cmd("vertex_elements", &[VERTEX_ELEMENTS_3, VE0[0], VE0[1], VE1[0], VE1[1], VE2[0], VE2[1]]);
    r.cmd("dummy_draw", &[PRIM, PRIM_TRILIST, 0, 0, 1, 0, 0]);
    r.cmd("3dprimitive", &[PRIM, PRIM_TRILIST, 6, 0, 1, 0, 0]);
    r.cmd("fence", &[PC4, PC_POSTSYNC_FLAGS, state + OFF_FENCE, R11_FENCE]);
    if r.d % 2 == 1 {
        r.cmd("noop-pad", &[MI_NOOP]);
    }
    if r.d > 1024 || r.n >= MARKS {
        return Err("ring-length"); // nothing written to the GT yet
    }
    let tail = (r.d * 4) as u32;
    clflush_range(base_va, n * 4096);
    let fence = (va(PG_STATE) + OFF_FENCE as usize) as *mut u32;

    let mut out = Out11 { cand, tail, ..Out11::default() };

    // ---- claim: PTEs, landed check, the pinned GTT flush (R10's) -----------------------------------------------------
    for k in 0..n {
        wr(bar0, slot(k), ptes[k]);
    }
    out.landed = (0..n).all(|k| rd(bar0, slot(k)) == ptes[k]) && rd(bar0, prev_off) == prev && rd(bar0, next_off) == next;
    wr(bar0, flsh, 1);

    out.disabled = true;
    if out.landed {
        let hold = fw_acquire(bar0, "r11", cand, class_pin, req_off, ack_off, ack_mask, srcp, mask_form, FW_ACK_BUDGET_CYC);
        wr(bar0, g7regs::RCS_RING_CTL, 0);
        wr(bar0, g7regs::RCS_RING_START, gtt(PG_RING));
        wr(bar0, g7regs::RCS_RING_HEAD, 0);
        wr(bar0, g7regs::RCS_RING_TAIL, 0);
        wr(bar0, g7regs::RCS_RING_CTL, RCS_RING_CTL_1PAGE_EN);
        out.armed = rd(bar0, g7regs::RCS_RING_CTL) & 1 == 1;
        if out.armed {
            core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
            let t0 = crate::arch::now_cycles();
            wr(bar0, g7regs::RCS_RING_TAIL, tail);
            let _ = poll_cycles(EXEC_BUDGET_CYC, || {
                clflush_range(fence as usize, 4);
                core::ptr::read_volatile(fence) == R11_FENCE
            });
            let (idle, _, _) = poll_cycles(DRAIN_BUDGET_CYC, || (rd(bar0, g7regs::RCS_RING_HEAD) & RING_HEAD_OFF_MASK) == tail);
            out.cyc = crate::arch::now_cycles().saturating_sub(t0);
            out.idle = idle;
            clflush_range(fence as usize, 4);
            out.read = core::ptr::read_volatile(fence);
            out.hit = out.read == R11_FENCE;
            out.head = rd(bar0, g7regs::RCS_RING_HEAD) & RING_HEAD_OFF_MASK;
        }
        // Teardown under the hold (R6's rule): disable, and restore the entry images only behind a disabled ring.
        wr(bar0, g7regs::RCS_RING_CTL, 0);
        out.disabled = rd(bar0, g7regs::RCS_RING_CTL) & 1 == 0;
        if out.disabled {
            wr(bar0, g7regs::RCS_RING_TAIL, tail_pre);
            wr(bar0, g7regs::RCS_RING_HEAD, head_pre);
            wr(bar0, g7regs::RCS_RING_START, start_pre);
            wr(bar0, g7regs::RCS_RING_CTL, ctl_pre);
            out.regs_restored = rd(bar0, g7regs::RCS_RING_START) == start_pre
                && rd(bar0, g7regs::RCS_RING_CTL) == ctl_pre
                && (rd(bar0, g7regs::RCS_RING_HEAD) & RING_HEAD_OFF_MASK) == (head_pre & RING_HEAD_OFF_MASK)
                && rd(bar0, g7regs::RCS_RING_TAIL) == tail_pre;
        }
        let (fw_ok, _ev) = fw_release(bar0, "r11", &hold);
        out.fw_restored = fw_ok;
    }
    out.passed = r.passed(out.head);

    // ---- the read-back and the CPU composite: every RT pixel against src[y / scale][x / scale] ----------------------
    clflush_range(va(pg_rt), g.rt_pages() * 4096);
    for y in 0..g.rt_h {
        let srow = (y / g.scale) * g.w;
        for x in 0..g.rt_w {
            let v = core::ptr::read_volatile((va(pg_rt) + y * g.rt_pitch + x * 4) as *const u32);
            let want = src[srow + x / g.scale];
            if v != R11_SEED {
                out.pixels += 1;
            }
            if v != want {
                out.mismatch += 1;
                if out.first.is_none() {
                    out.first = Some((x as u32, y as u32, v, want));
                }
            }
        }
    }

    // ---- unwind the window: only behind a quiesced engine (a ring that did not drain may still write) --------------
    if out.disabled && (out.idle || !out.armed) {
        for k in 0..n {
            wr(bar0, slot(k), base_img);
        }
        out.ptes_restored = (0..n).all(|k| rd(bar0, slot(k)) == base_img) && rd(bar0, prev_off) == prev && rd(bar0, next_off) == next;
        wr(bar0, flsh, 1);
    }
    Ok(out)
}
