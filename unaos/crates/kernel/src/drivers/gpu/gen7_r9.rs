//! GEN7R9 (B504, R101) — rung R9: the first 3D-pipeline step on the Ivy Bridge RCS; GEN7R9B (B626) makes it execute.
//! CHARTER: Kernel — driver
//!
//! Design: `docs/dev/evidence/rmbp-1005/gen7r9.md` (B504) and `docs/dev/evidence/gen7r9b/GEN7R9B.md` (B626: the row
//! table, command -> dwords -> source line). A child of `gen7` (declared by `#[path]` at the tail of gen7.rs, as
//! `gen7_blit.rs` is), run by `tests gen7` right after R1..R7 (R80: nothing here runs at boot).
//!
//! THE RUNG: on the RCS (not the BCS), `PIPELINE_SELECT(3D)`, `STATE_BASE_ADDRESS` (every base = the GGTT page the
//! rung pins, modify bits set), and one `PIPE_CONTROL` whose post-sync operation writes an immediate into that page —
//! the immediate landing is the proof the 3D pipeline executed a command stream, the shape every later 3D rung needs.
//!
//! THE RING HALF IS R6's. Flight 27 (`f27-boot1.log`): `gen7: r6 verdict=r6-sentinel-hit by=mt …` — R6 armed the RCS
//! (0x2030..0x203C) in a one-page ring under R3's held `mt` wake and executed MI_STORE_DATA_IMM. R9 is GATED on that
//! verdict this run, never on a forcewake ack decode (§4/R3), and arms the RCS the way R6 and GEN7B's `bcs_job` do,
//! with the SAME helpers (`rd`/`wr`, `fw_acquire`/`fw_release` on R6's `mt` row, `poll_cycles`, `clflush_range`).
//!
//! THE 3D HALF IS PINNED TO THE LINUX i915 DRIVER (B626). Intel's PRM hosts answer 403 through the proxy; the i915
//! files below are MIT-licensed and are read for HARDWARE FACTS only — opcodes, dword counts, bit fields — under the
//! R95 §2 / R110 precedent, each cited file:line (torvalds/linux master 24a19b95, fetched 2026-10-13). No code is
//! copied. `h` = drivers/gpu/drm/i915/gt/intel_gpu_commands.h, `rc` = gt/gen7_renderclear.c, `rs` =
//! gt/gen7_renderstate.c, `cs` = gt/gen6_engine_cs.c. R83 / R95 §2: a row whose `src` is `None` DECLINES the rung by
//! name and nothing is written; every R9 row is filled. R10 (the first draw, GEN7R10/B638) is `gen7_r10.rs`.

use super::*;

/// One command a rung needs, and the source that pins its encoding (`None` = uncited).
struct Page {
    /// The command's PRM name, printed in the decline reason (`page-<name>-uncited`).
    name: &'static str,
    /// The witness field this row answers.
    field: &'static str,
    /// The PRM volume the row is owed from (gen7b.md §4), for the reader of the wire.
    owed: &'static str,
    /// The transcription that pins the encoding. `None` until a page is in the tree.
    src: Option<&'static str>,
}

/// R9's citation table, in command-stream order (`GEN7R9B.md` §2 carries the same rows with every dword).
const PAGES: [Page; 6] = [
    Page { name: "PIPE_CONTROL-stall", field: "stall", owed: "IVB-PRM-Vol2-Pt1", src: Some("h:287,295,317 cs:280-283 rc:360-365") },
    Page { name: "PIPE_CONTROL-invalidate", field: "invalidate", owed: "IVB-PRM-Vol2-Pt1", src: Some("h:287,316 rc:367-371") },
    Page { name: "PIPELINE_SELECT", field: "select", owed: "IVB-PRM-Vol2-Pt1", src: Some("h:405-407 rs:19 rc:413-414") },
    Page { name: "PIPE_CONTROL-flush", field: "flush", owed: "IVB-PRM-Vol2-Pt1", src: Some("h:287,295,306,313,318 rc:344-349") },
    Page { name: "STATE_BASE_ADDRESS", field: "sba", owed: "IVB-PRM-Vol2-Pt1", src: Some("h:402-404 rs:20 rc:240-256") },
    Page { name: "PIPE_CONTROL", field: "pipe_control", owed: "IVB-PRM-Vol1-Pt3", src: Some("h:287,292,295,301,306,312,313,318 cs:355-364") },
];

// ---- The encodings, each a hardware fact with its line (GEN7R9B.md §2) -------------------------------------------
/// `GFX_OP_PIPE_CONTROL(len)` = 3<<29 | 3<<27 | 2<<24 | (len-2) — h:287.
const fn pipe_control(len: u32) -> u32 {
    (0x3 << 29) | (0x3 << 27) | (0x2 << 24) | (len - 2)
}
const PC4: u32 = pipe_control(4); // 0x7A000002 — cs:280, cs:355, rc:344
const PC5: u32 = pipe_control(5); // 0x7A000003 — rc:360, rc:367
const PC_DEPTH_CACHE_FLUSH: u32 = 1 << 0; // h:318
const PC_STALL_AT_SCOREBOARD: u32 = 1 << 1; // h:317
const PC_STATE_CACHE_INVALIDATE: u32 = 1 << 2; // h:316
const PC_DC_FLUSH_ENABLE: u32 = 1 << 5; // h:313
const PC_FLUSH_ENABLE: u32 = 1 << 7; // h:312 (gen7+)
const PC_RENDER_TARGET_CACHE_FLUSH: u32 = 1 << 12; // h:306 (gen6+)
const PC_POST_SYNC_WRITE_IMM: u32 = 1 << 14; // h:301 `PIPE_CONTROL_QW_WRITE`; h:302 names bits 15:14 the post-sync op
const PC_CS_STALL: u32 = 1 << 20; // h:295
const PC_GLOBAL_GTT_IVB: u32 = 1 << 24; // h:292 (gen7+): the post-sync address is a GGTT address
/// `PIPELINE_SELECT` = 3<<29 | 1<<27 | 1<<24 | 4<<16 (h:405-406); bit 0 is MEDIA (h:407), so 3D is bit 0 clear —
/// the exact dword the gen7 golden render state opens with (rs:19, `0x69040000`).
const PIPELINE_SELECT_3D: u32 = (0x3 << 29) | (0x1 << 27) | (0x1 << 24) | (0x4 << 16);
/// `STATE_BASE_ADDRESS` = 3<<29 | 0<<27 | 1<<24 | 1<<16 (h:402-403), length 10 dwords (rc:240 `| (10 - 2)`; rs:20).
const SBA_DW0: u32 = (0x3 << 29) | (0x1 << 24) | (0x1 << 16) | (10 - 2);
/// `BASE_ADDRESS_MODIFY` = bit 0 of each base / bound dword (h:404; rc:242-256).
const BASE_ADDRESS_MODIFY: u32 = 1 << 0;
// The literal dwords i915 carries are the values these constants build (rs:19, rs:20; h:287 at len 4).
const _: () = assert!(PIPELINE_SELECT_3D == 0x6904_0000 && SBA_DW0 == 0x6101_0008 && PC4 == 0x7A00_0002);

/// The post-sync flags: i915's gen7 RCS breadcrumb, bit for bit (cs:356-362).
const PC_POSTSYNC_FLAGS: u32 = PC_RENDER_TARGET_CACHE_FLUSH
    | PC_DEPTH_CACHE_FLUSH
    | PC_DC_FLUSH_ENABLE
    | PC_FLUSH_ENABLE
    | PC_POST_SYNC_WRITE_IMM
    | PC_GLOBAL_GTT_IVB
    | PC_CS_STALL;
/// renderclear's flush (rc:345-348).
const PC_FLUSH_FLAGS: u32 = PC_RENDER_TARGET_CACHE_FLUSH | PC_DEPTH_CACHE_FLUSH | PC_DC_FLUSH_ENABLE | PC_CS_STALL;

// ---- The window -------------------------------------------------------------------------------------------------
/// The GGTT inside BAR0 [ONE-SOURCE: capture — R4..R8 `ptes_landed=1` on metal], as R6 and `gen7_blit.rs` carry it.
const GTT_BASE: usize = 0x200000;
const GGTT_SLOTS: usize = 524_288;
/// R9's own two-slot window: slot 0 the ring page, slot 1 the pinned page (every SBA base and the post-sync target).
/// GGTT 0x13400000: RING_START bits[31:29] zero (Vol1 Pt3 §1.1.11.3 p.77), above GEN7B's window (0x11000 + ≤ 8192
/// slots = 0x13000) and R5..R8's (0x10000..). Proven unowned at run time before any write, or the rung refuses.
const R9_BASE_SLOT: usize = 0x13400;
const R9_WIN: usize = 2;
/// The immediate the post-sync writes, and the seed the CPU leaves there first, so a hit is a transition.
const R9_IMMEDIATE: u32 = 0x3D09_C0DE;
const R9_SEED: u32 = 0x5EED_3D09;
/// Byte offset of the post-sync dword in the pinned page: QWord aligned (the page is R9's alone).
const R9_POST_OFF: usize = 0x40;
/// The ring, in dwords: (end dword exclusive, name) — the head offset maps onto the last command it retired.
const RING_CMDS: [(usize, &str); 8] = [
    (10, "invalidate-0"),
    (12, "pipeline_select"),
    (22, "invalidate-1"),
    (26, "flush"),
    (36, "state_base_address"),
    (46, "invalidate-2"),
    (50, "pipe_control"),
    (52, "noop-pad"),
];
const RING_DW: usize = 52; // 208 bytes: QWord aligned (§1.1.11.1 p.75)

/// The ring + pinned pages (8 KiB), allocated once and held for the boot (GEN7TLB, gen7.md §2.6: a page the GT may
/// hold a translation to never goes back to the heap on an unpinned register's word).
static PAGES_VA: crate::sync::Mutex<usize> = crate::sync::Mutex::new(0);

/// R6's verdict this run (`rearm` notes it at its verdict line).
static R6: crate::sync::Mutex<Option<&'static str>> = crate::sync::Mutex::new(None);
/// R9's one-word result this boot, for the `GEN7LADDER` line and the replay.
static WORD: crate::sync::Mutex<Option<&'static str>> = crate::sync::Mutex::new(None);

/// Called by R6 (`rearm`) with its final verdict.
pub(super) fn note_r6(v: &'static str) {
    *R6.lock() = Some(v);
}

/// The `r9=` word on the `GEN7LADDER` line (`verified`, `verified-unrestored`, `failed`, `refused`, `declined`,
/// `gated`, or `not-reached` before R9 ran).
pub(super) fn word() -> &'static str {
    (*WORD.lock()).unwrap_or("not-reached")
}

/// What the RCS run did. Every field a reading.
#[derive(Clone, Copy, Default)]
struct Out {
    cand: &'static str,
    landed: bool,
    armed: bool,
    hit: bool,
    read: u32,
    head: u32,
    idle: bool,
    disabled: bool,
    regs_restored: bool,
    fw_restored: bool,
    ptes_restored: bool,
    cyc: u64,
}

fn cycles_to_us(c: u64) -> u64 {
    let hz = crate::arch::apic::tsc_hz();
    let hz = if hz == 0 { 2_500_000_000 } else { hz };
    c.saturating_mul(1_000_000) / hz
}

/// The last command the head had retired: the last `RING_CMDS` row whose end is at or below the head's dword.
fn passed(head: u32) -> &'static str {
    let dw = (head as usize) / 4;
    let mut last = "none";
    for &(end, name) in RING_CMDS.iter() {
        if end <= dw {
            last = name;
        }
    }
    last
}

/// R9 under `tests gen7`: gate on R6, then the citation table, then the RCS run; one `[gen7] r9` witness line, then
/// R10's one `[gen7] r10` line.
pub(super) fn run() {
    let r6 = (*R6.lock()).unwrap_or("not-reached");
    let mut cited = [""; 3];
    for (i, f) in ["select", "sba", "pipe_control"].iter().enumerate() {
        cited[i] = match PAGES.iter().find(|p| p.field == *f) {
            Some(p) if p.src.is_some() => "cited",
            _ => "uncited",
        };
    }
    if r6 != "r6-sentinel-hit" {
        *WORD.lock() = Some("gated");
        serial_println!(
            "[gen7] r9 ring=rcs r6={} select={} sba={} pipe_control={} sync_val=- read=- writes=0 -> GATED reason=r6-did-not-execute-on-the-rcs",
            r6, cited[0], cited[1], cited[2]
        );
        r10(word());
        return;
    }
    // The first row without a source declines the rung, by name (R83: no bit guessed).
    if let Some(p) = PAGES.iter().find(|p| p.src.is_none()) {
        *WORD.lock() = Some("declined");
        serial_println!(
            "[gen7] r9 ring=rcs r6={} select={} sba={} pipe_control={} sync_val=- read=- writes=0 -> DECLINED reason=page-{}-uncited field={} owed={}",
            r6, cited[0], cited[1], cited[2], p.name, p.field, p.owed
        );
        r10(word());
        return;
    }
    let bank = *super::ladder::BANK.lock();
    let Some(b) = bank else {
        *WORD.lock() = Some("refused");
        serial_println!("[gen7] r9 ring=rcs r6={} pipeline=- writes=0 -> REFUSED reason=no-boot-bank", r6);
        r10(word());
        return;
    };
    // SAFETY: `bar0` is the IGD BAR0 mapping `igpu::init` published and banked; never unmapped. `exec` writes only
    // its own proven-unowned GGTT window, `0x101008`, one forcewake request (restored) and the four RCS ring
    // registers (restored), and refuses before any write on every doubt it can read.
    let t0 = crate::arch::now_cycles();
    let res = unsafe { exec(b.bar0, b.bar0_size) };
    let us = cycles_to_us(crate::arch::now_cycles().saturating_sub(t0));
    let o = match res {
        Err(why) => {
            *WORD.lock() = Some("refused");
            serial_println!("[gen7] r9 ring=rcs r6={} pipeline=- writes=0 -> REFUSED reason={} us={}", r6, why, us);
            r10(word());
            return;
        }
        Ok(o) => o,
    };
    let tail = (RING_DW * 4) as u32;
    let restored = o.regs_restored && o.fw_restored && o.ptes_restored;
    //  r9-3d-postsync-verified        THE WIN: the immediate landed and the head retired the whole stream.
    //  r9-postsync-landed-head-stuck  the write landed, the head did not reach the tail.
    //  r9-postsync-miss               the head retired the stream and no write landed: suspect the post-sync
    //                                 address / GGTT bit (cs:361-363), not the pipeline select.
    //  r9-head-stuck                  the CS stopped inside the stream; `passed=` names the last command retired.
    //  r9-enable-void / r9-claim-write-void / r9-ring-would-not-disable   R6's envelope verdicts, same meaning.
    let verdict = if !o.landed {
        "r9-claim-write-void"
    } else if !o.armed {
        "r9-enable-void"
    } else if !o.disabled {
        "r9-ring-would-not-disable"
    } else if o.hit && o.idle {
        "r9-3d-postsync-verified"
    } else if o.hit {
        "r9-postsync-landed-head-stuck"
    } else if o.head == tail {
        "r9-postsync-miss"
    } else {
        "r9-head-stuck"
    };
    let ok = verdict == "r9-3d-postsync-verified";
    *WORD.lock() = Some(if ok && restored { "verified" } else if ok { "verified-unrestored" } else { "failed" });
    let head_dw = (o.head as usize) / 4;
    let select_retired = o.armed && head_dw >= RING_CMDS[1].0;
    let sba_retired = o.armed && head_dw >= RING_CMDS[4].0;
    serial_println!(
        "[gen7] r9 ring=rcs r6={} cand={} pipeline={} sba={} pipe_control={} immediate=0x{:08X} read=0x{:08X} landed={} head={:#x} tail={:#x} passed={} idle={} restored=regs:{},fw:{},ptes:{} gpu_us={} us={} -> {}",
        r6,
        o.cand,
        if select_retired { "3d" } else { "not-retired" },
        if sba_retired { "ok" } else { "not-retired" },
        if o.hit { "posted" } else { "unposted" },
        R9_IMMEDIATE,
        o.read,
        o.hit as u32,
        o.head,
        tail,
        passed(o.head),
        o.idle as u32,
        o.regs_restored as u32,
        o.fw_restored as u32,
        o.ptes_restored as u32,
        cycles_to_us(o.cyc),
        us,
        verdict
    );
    r10(word());
}

/// Append one dword to the ring page at `*d`.
unsafe fn put(ring: *mut u32, d: &mut usize, v: u32) {
    core::ptr::write_volatile(ring.add(*d), v);
    *d += 1;
}

/// `gen7_emit_pipeline_invalidate`'s shape (rc:355-374): a CS stall at the scoreboard, then the state-cache
/// invalidate (the stall-before-invalidate workaround, cs:333-337). 10 dwords.
unsafe fn invalidate(ring: *mut u32, d: &mut usize) {
    for v in [PC5, PC_STALL_AT_SCOREBOARD | PC_CS_STALL, 0, 0, 0, PC5, PC_STATE_CACHE_INVALIDATE, 0, 0, 0] {
        put(ring, d, v);
    }
}

/// The RCS run. `Err(why)` before any write; `Ok(out)` once the window was claimed.
///
/// # Safety
/// `bar0`/`bar0_size` are the IGD BAR0 mapping the ladder banked. Writes: the two window PTEs (restored), `0x101008`
/// (write-any-value, gen7.md §2.6), one forcewake request register (restored by `fw_release`), the four RCS ring
/// registers (restored to their entry images, only behind a disabled ring). No display register.
unsafe fn exec(bar0: usize, bar0_size: usize) -> Result<Out, &'static str> {
    let flsh = g7regs::HYP_GFX_FLSH_CNTL;
    let need = (GTT_BASE + (R9_BASE_SLOT + R9_WIN + 1) * 4).max(flsh + 4).max(g7regs::IVB_FORCEWAKE_MT_ACK + 4);
    if R9_BASE_SLOT + R9_WIN + 1 >= GGTT_SLOTS || bar0_size < need {
        return Err("bar0-too-small");
    }
    let Some(&(cand, class_pin, req_off, ack_off, ack_mask, srcp, mask_form)) = R6_CANDS.iter().find(|c| c.0 == "mt") else {
        return Err("no-mt-row");
    };

    // ---- census: the window and both neighbours, before any write (R4b's legs, as `bcs_job` reads them) ----------
    let slot = |k: usize| GTT_BASE + (R9_BASE_SLOT + k) * 4;
    let prev_off = GTT_BASE + (R9_BASE_SLOT - 1) * 4;
    let next_off = slot(R9_WIN);
    let fill = rd(bar0, slot(0));
    let (prev, next) = (rd(bar0, prev_off), rd(bar0, next_off));
    if prev != fill || next != fill || (0..R9_WIN).any(|k| rd(bar0, slot(k)) != fill) {
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

    // ---- the pages: ring + pinned, held for the boot ----------------------------------------------------------------
    let ring_va = {
        let mut pages = PAGES_VA.lock();
        if *pages == 0 {
            let p = alloc_zeroed(Layout::from_size_align(8192, 4096).unwrap());
            if p.is_null() {
                return Err("alloc");
            }
            *pages = p as usize;
        }
        *pages
    };
    let pin_va = ring_va + 4096;
    let mut ptes = [0u32; R9_WIN];
    for (k, va) in [ring_va, pin_va].iter().enumerate() {
        let Some(pa) = crate::arch::memory::translate(*va as u64) else { return Err("va-unmapped") };
        if pa >= 0x1_0000_0000 {
            return Err("phys-above-4g"); // the 32-bit PTE form R4..R8 flew
        }
        ptes[k] = (pa as u32 & 0xFFFF_F000) | GGTT_PTE_VALID;
        if ptes[k] == base_img {
            return Err("pte-indistinct");
        }
    }
    let gtt = |k: usize| ((R9_BASE_SLOT + k) * 4096) as u32;
    let pin_gtt = gtt(1);
    let post_gtt = pin_gtt + R9_POST_OFF as u32;

    // ---- the ring (GEN7R9B.md §2): renderclear's invalidate/flush brackets around the select and the SBA
    // (rc:412-419), then i915's gen7 RCS breadcrumb as the post-sync (cs:355-364), MI_NOOP for its MI_USER_INTERRUPT.
    let ring = ring_va as *mut u32;
    for i in 0..1024 {
        core::ptr::write_volatile(ring.add(i), MI_NOOP);
    }
    let mut d = 0usize;
    invalidate(ring, &mut d); // dw 0..10 (rc:412)
    put(ring, &mut d, PIPELINE_SELECT_3D); // dw 10 (rc:413 selects MEDIA; 3D = bit 0 clear, rs:19)
    put(ring, &mut d, MI_NOOP); // dw 11 (rc:414)
    invalidate(ring, &mut d); // dw 12..22 (rc:415)
    for v in [PC4, PC_FLUSH_FLAGS, 0, 0] {
        put(ring, &mut d, v); // dw 22..26 (rc:417, rc:344-349)
    }
    // dw 26..36: STATE_BASE_ADDRESS in rc:240-256's layout — general, surface, dynamic, indirect, instruction bases,
    // then the general/dynamic/indirect/instruction access upper bounds — every base the pinned page.
    put(ring, &mut d, SBA_DW0);
    for _ in 0..5 {
        put(ring, &mut d, pin_gtt | BASE_ADDRESS_MODIFY);
    }
    for v in [0, BASE_ADDRESS_MODIFY, 0, BASE_ADDRESS_MODIFY] {
        put(ring, &mut d, v);
    }
    invalidate(ring, &mut d); // dw 36..46 (rc:419)
    for v in [PC4, PC_POSTSYNC_FLAGS, post_gtt, R9_IMMEDIATE] {
        put(ring, &mut d, v); // dw 46..50 (cs:355-364)
    }
    put(ring, &mut d, MI_NOOP); // dw 50..52: QWord pad
    put(ring, &mut d, MI_NOOP);
    if d != RING_DW {
        return Err("ring-length"); // the RING_CMDS map and the stream disagree: nothing written to the GT yet
    }
    let tail = (RING_DW * 4) as u32;
    let post = (pin_va + R9_POST_OFF) as *mut u32;
    core::ptr::write_volatile(post, R9_SEED);
    clflush_range(ring_va, 8192);

    let mut out = Out { cand, ..Out::default() };

    // ---- claim: PTEs, landed check, the pinned GTT flush (Vol1 Pt3 §1.2.21 p.191) -----------------------------------
    for k in 0..R9_WIN {
        wr(bar0, slot(k), ptes[k]);
    }
    out.landed = (0..R9_WIN).all(|k| rd(bar0, slot(k)) == ptes[k]) && rd(bar0, prev_off) == prev && rd(bar0, next_off) == next;
    wr(bar0, flsh, 1);

    out.disabled = true;
    if out.landed {
        let hold = fw_acquire(bar0, "r9", cand, class_pin, req_off, ack_off, ack_mask, srcp, mask_form, FW_ACK_BUDGET_CYC);
        wr(bar0, g7regs::RCS_RING_CTL, 0);
        wr(bar0, g7regs::RCS_RING_START, gtt(0));
        wr(bar0, g7regs::RCS_RING_HEAD, 0);
        wr(bar0, g7regs::RCS_RING_TAIL, 0);
        wr(bar0, g7regs::RCS_RING_CTL, RCS_RING_CTL_1PAGE_EN);
        out.armed = rd(bar0, g7regs::RCS_RING_CTL) & 1 == 1;
        if out.armed {
            core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
            let t0 = crate::arch::now_cycles();
            wr(bar0, g7regs::RCS_RING_TAIL, tail);
            let _ = poll_cycles(EXEC_BUDGET_CYC, || {
                clflush_range(post as usize, 4);
                core::ptr::read_volatile(post) == R9_IMMEDIATE
            });
            let (idle, _, _) = poll_cycles(DRAIN_BUDGET_CYC, || (rd(bar0, g7regs::RCS_RING_HEAD) & RING_HEAD_OFF_MASK) == tail);
            out.cyc = crate::arch::now_cycles().saturating_sub(t0);
            out.idle = idle;
            clflush_range(post as usize, 4);
            out.read = core::ptr::read_volatile(post);
            out.hit = out.read == R9_IMMEDIATE;
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
        let (fw_ok, _ev) = fw_release(bar0, "r9", &hold);
        out.fw_restored = fw_ok;
    }

    // ---- unwind the window: only behind a quiesced engine (a ring that did not drain may still write) --------------
    if out.disabled && (out.idle || !out.armed) {
        for k in 0..R9_WIN {
            wr(bar0, slot(k), base_img);
        }
        out.ptes_restored = (0..R9_WIN).all(|k| rd(bar0, slot(k)) == base_img) && rd(bar0, prev_off) == prev && rd(bar0, next_off) == next;
        wr(bar0, flsh, 1);
    }
    Ok(out)
}

// =====================================================================================================================
// R10 — the first DRAW (GEN7R10, B638). Its row table, stream and read-back live in `gen7_r10.rs`, a child of this
// file so it reuses R9's encodings and helpers; GEN7R10.md carries every row page-cited against the IVB PRM.
// =====================================================================================================================

#[path = "gen7_r10.rs"]
mod draw;

/// R10 under `tests gen7`, after R9's witness line: gated on R9's word, one `[gen7] r10` line.
fn r10(r9: &str) {
    draw::run(r9)
}
