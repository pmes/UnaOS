//! GA10B-PROBE5A — RUNG 5a of the GA10B ladder: the VENDOR IGNITION (`ga10bprobe5a`, DEFAULT OFF;
//! `UNAOS_GA10B_PROBE5=1` raw addresses, `=5` addresses `pa >> 8` under `ga10bprobe5e`; `=2`/`=7` add rung
//! 5b under `ga10bprobe5b`). Design: docs/dev/OS/08_VIDEO/GA10B-RUNG5-BRIEF.md §5 (as built); rung 4's
//! brief §3/§4/§10 for the writes and the oracles this reuses; ledger A63 (5a) and A64 (5b).
//!
//! THE RUNG, in order, on ONE boot:
//!   0. ZERO-MMIO PHASE — the rung-4 Normal-NC window (seated at heap-guard, `[ga10b4nc]`) is re-mapped
//!      NC and filled with the rung-4 pattern; `ga10b_fw::load` reads the three vendor files from
//!      `/boot/GA10B/` through the VFS, sizes them, places them 256-byte-aligned, digests what landed.
//!      ANY refusal here RETURNS with zero MMIO and the boot continues into the desktop (brief §5.6).
//!   1. P0–P2 — rung 4a's bracket re-proven in this family: gpu@ node from the DTB, BPMP MRQ_PG with an
//!      explicit readback, the clocks, `bcr_dmacfg` lock bit 0, `bcr_ctrl` baseline.
//!   2. P5 — rung 4a's seven-write census + restore, re-run here (BCR-ALLHELD this boot or no ignition).
//!   3. Rung 4c's PRE-ignition census (30 registers, read-only).
//!   4. THE IGNITION — rung 4b's seven writes with ONLY THE PAYLOAD changed: the six BCR addresses point at
//!      the three PLACED sections (raw, or `>> 8` under 5e), `bcr_dmacfg` = noncoherent | lock (SPENDS THE
//!      POWER CYCLE), `bcr_ctrl` = 0x111, `priscv_cpuctl` = startcpu, the bounded `br_retcode` poll.
//!   5. The ORACLES — post-ignition cpuctl / hwcfg2 lockdown / v1 mirror; the br_retcode SERIES; POSTBCR
//!      (the 8 registers against what was written); MAPDIFF (the 30 registers against pass 3); the window
//!      re-digested per section and first-words-sampled (did anything WRITE our buffer — a READ leaves no
//!      trace and the wire says so); MAILBOX0 read back with its meaning decided in code.
//!   6. THE VERDICT — `ACR-ACCEPTED` (lockdown dropped OR the v1 mirror readable — a GPU-side change no
//!      unsigned payload has ever produced) | `BROM-VERDICT-FAIL code=` | `BROM-VERDICT-PASS-UNWITNESSED
//!      code=` (F20: the code says PASS and no oracle moved — a measurement error until re-flown) |
//!      `BROM-NOVERDICT`; then rung 5b if armed; then SYSTEM_OFF (the lock made the BCR final; cold boot next).
//!
//! WHAT IT REUSES. Rung 4a's helpers from `ga10b_probe.rs` — `r32`, `w32_4`, `bcr_write_verify`,
//! `unreadable_reason`, `finish4`, `resolve_gpu_node`, `pg_state`, `clk`, `settle_ms`, `BCR_ADDR_REGS` —
//! made `pub(crate)` for this module (one-line visibility changes, nothing else in that file). Every offset
//! below is the ACKED facts file's (`ga10b-probe-rung1.facts.md` §(b)) except MAILBOX0, PUBLIC-RECALLED and
//! metal-proven by rung 3b — the same provenance rung 4c states for the same list.
//!
//! WHAT IT NEVER DOES (brief §5.5): no copy, blit, triangle or pixel; no WPR / MC GSC / carveout register;
//! no PMU ignition, no FECS/GPCCS, no second triple; nothing decrypts, inspects, patches or renames a
//! vendor byte. It spends the power cycle by design and does not claim the board is left as found.
//!
//! WITNESS FAMILY `[ga10bprobe5a]` — 15 bytes bracketed; `LC_ALL=C grep -a -o -F` certifies the artifact.

use super::ga10b_probe::{
    bcr_write_verify, clk, finish4, pg_state, r32, resolve_gpu_node, settle_ms, unreadable_reason, w32_4,
    BCR_ADDR_REGS,
};
use super::ga10b_fw::{self, Outcome, Placed, ACR_GSP, ROLE};

const FAM: &str = "ga10bprobe5a";

// ── Facts (ga10b-probe-rung1.facts.md §(b); the same numbers rung 3/4 read on this die) ────────────────
const GSP_FALCON_BASE: u64 = 0x0011_0000;
const GSP_FALCON2_BASE: u64 = 0x0011_1000;
const PMU_FALCON2_BASE: u64 = 0x0010_b000;
const FUSE_OPT_SEC_DEBUG_EN: u64 = 0x0082_1040;
const FUSE_OPT_WPR_ENABLED: u64 = 0x0082_05ec;
const FUSE_OPT_VPR_ENABLED: u64 = 0x0082_067c;
const MC_ENABLE: u64 = 0x0000_0200;
const MC_ELPG_ENABLE: u64 = 0x0000_020c;
const TOP_DEVICE_INFO_CFG: u64 = 0x0002_24fc;
const TOP_NUM_GPCS: u64 = 0x0002_2430;
const FALCON_IRQMASK_OFF: u64 = 0x018;
const FALCON_IRQDEST_OFF: u64 = 0x01c;
/// PUBLIC-RECALLED (nouveau nvkm/falcon; open-gpu-kernel-modules dev_falcon_v4.h, MIT), metal-proven by
/// rung 3b (render11 MAILBOX-HELD) — NOT in the ACKED facts file. Read only, here.
const FALCON_MAILBOX0_OFF: u64 = 0x040;
const FALCON_IDLESTATE_OFF: u64 = 0x04c;
const FALCON_HWCFG2_OFF: u64 = 0x0f4;
const HWCFG2_PRIV_LOCKDOWN_BIT: u32 = 13;
const FALCON_CPUCTL_OFF: u64 = 0x100;
const FALCON_HWCFG_OFF: u64 = 0x108;
const FALCON_DMACTL_OFF: u64 = 0x10c;
const PRISCV_BOOT_VECTOR_LO_OFF: u64 = 0x380;
const PRISCV_BOOT_VECTOR_HI_OFF: u64 = 0x384;
const PRISCV_CPUCTL_OFF: u64 = 0x388;
const PRISCV_CPUCTL_HALTED_BIT: u32 = 4;
const PRISCV_RISCV_IRQMASK_OFF: u64 = 0x528;
const PRISCV_RISCV_IRQDEST_OFF: u64 = 0x52c;
const PRISCV_BR_RETCODE_OFF: u64 = 0x65c;
const BR_RETCODE_FAIL: u32 = 0x2;
const BR_RETCODE_PASS: u32 = 0x3;
const PRISCV_BCR_CTRL_OFF: u64 = 0x668;
const PRISCV_BCR_DMACFG_OFF: u64 = 0x66c;
const PRISCV_BCR_PKCPARAM_LO_OFF: u64 = 0x670;
const PRISCV_BCR_PKCPARAM_HI_OFF: u64 = 0x674;
const PRISCV_BCR_FMCCODE_LO_OFF: u64 = 0x678;
const PRISCV_BCR_FMCCODE_HI_OFF: u64 = 0x67c;
const PRISCV_BCR_FMCDATA_LO_OFF: u64 = 0x680;
const PRISCV_BCR_FMCDATA_HI_OFF: u64 = 0x684;
const BCR_DMACFG_LOCK_LOCKED: u32 = 0x8000_0000;
const BCR_DMACFG_TARGET_NONCOHERENT_SYSTEM: u32 = 0x2;
/// facts (b) SEQ step 1: brom_config bcr_ctrl = 0x111 (BRFETCH TRUE | CORE_SELECT RISCV | VALID TRUE).
const BCR_CTRL_BROM_CONFIG: u32 = 0x111;
/// facts (b): priscv cpuctl 0x388 startcpu_true = 0x1 — THE IGNITION.
const PRISCV_CPUCTL_STARTCPU: u32 = 0x1;
const BR_POLL_SAMPLES: u32 = 16;
const BR_POLL_SETTLE_MS: u64 = 10;
const BR_SERIES_SAMPLES: u32 = 20;
const BR_SERIES_SETTLE_MS: u64 = 10;
/// Rung 4a's non-signature fill pattern: the window's unwritten tail stays this, never uninitialised RAM.
const DMABUF_PATTERN: u32 = 0x4A10_B4A5;
// BPMP (rung 2/3/4's shape).
const MRQ_PG: u32 = 66;
const CMD_PG_GET_STATE: u32 = 2;
const CMD_PG_SET_STATE: u32 = 1;
const PG_STATE_ON: u32 = 1;
const PG_STATE_OFF: u32 = 0;
const CMD_CLK_IS_ENABLED: u32 = 6;
const CMD_CLK_ENABLE: u32 = 7;
const CMD_CLK_DISABLE: u32 = 8;

/// The address ENCODING this build writes into the six BCR DMA registers: 0 = the raw physical address
/// (the flown rung-4b form); 8 = 256-byte units (NVIDIA's published MIT Hopper bootstrap, rung-5 brief
/// §1.2), the form rung 4e flies to settle. Both exist so 5a can fly whichever 4e proves (P5x).
#[cfg(not(feature = "ga10bprobe5e"))]
const ADDR_SHIFT: u32 = 0;
#[cfg(feature = "ga10bprobe5e")]
const ADDR_SHIFT: u32 = 8;
#[cfg(not(feature = "ga10bprobe5e"))]
const ADDR_FORM: &str = "raw";
#[cfg(feature = "ga10bprobe5e")]
const ADDR_FORM: &str = "shift8";

/// Rung 5a's OWN write helper for the lock, the trigger and the ignition — `#[cfg(feature = "ga10bprobe5a")]`
/// by the module's own gate, so no other configuration compiles it. The census writes go through rung 4a's
/// `bcr_write_verify` (announce + readback, one result line per announce).
#[inline]
fn ignite5_w32(pa: u64, v: u32) {
    unsafe { core::ptr::write_volatile(pa as *mut u32, v) }
}

/// Encode a physical address for the BCR: (lo, hi) halves of `pa >> ADDR_SHIFT`.
fn enc(pa: u64) -> (u32, u32) {
    let v = pa >> ADDR_SHIFT;
    (v as u32, (v >> 32) as u32)
}

/// The six address VALUES for the three placements, in BCR_ADDR_REGS order (fmccode lo/hi, fmcdata lo/hi,
/// pkcparam lo/hi), and the raw address behind each (for the announce).
fn addr_values(placed: &[Placed; 3]) -> ([u32; 6], [u64; 6]) {
    let (fcl, fch) = enc(placed[0].pa);
    let (fdl, fdh) = enc(placed[1].pa);
    let (pkl, pkh) = enc(placed[2].pa);
    (
        [fcl, fch, fdl, fdh, pkl, pkh],
        [placed[0].pa, placed[0].pa, placed[1].pa, placed[1].pa, placed[2].pa, placed[2].pa],
    )
}

/// An announced address write + readback carrying the RAW address and the encoding beside the value
/// written — `bcr_write_verify`'s shape (one announce, exactly one result line) with two more fields.
fn addr_write_verify(name: &str, f2: u64, off: u64, val: u32, raw: u64, why: &str) -> Result<bool, &'static str> {
    let addr = f2 + off;
    serial_println!("[{}] about-to-WRITE {} reg={:#x} val={:#010x} raw_pa={:#010x} form={} shift={} ({}) — if this is the LAST line, THAT WRITE was fatal and the boot ended inside it", FAM, name, addr, val, raw, ADDR_FORM, ADDR_SHIFT, why);
    w32_4(addr, val);
    let got = r32(addr);
    match unreadable_reason(got) {
        Some(r) => {
            serial_println!("[{}] {} @{:#x} wrote={:#010x} read=-UNREADABLE reason={} val={:#010x} raw_pa={:#010x} form={} — NOT folded into held or not-held (F4)", FAM, name, off, val, r, got, raw, ADDR_FORM);
            Err(r)
        }
        None => {
            let held = got == val;
            serial_println!("[{}] {} @{:#x} wrote={:#010x} read={:#010x} raw_pa={:#010x} form={} held={}", FAM, name, off, val, got, raw, ADDR_FORM, held as u32);
            Ok(held)
        }
    }
}

/// One census register (rung 4c's list, rung 3's risk order).
struct Reg {
    name: &'static str,
    off: u64,
    class: &'static str,
}
const R5C_N: usize = 30;
const R5C_BCR_FIRST: usize = 15;
const R5C_BCR_LAST: usize = 22;
const R5C_BR_RETCODE: usize = 28;
const R5C_MAILBOX0: usize = 14;
const CENSUS: [Reg; R5C_N] = [
    Reg { name: "fuse_opt_sec_debug_en", off: FUSE_OPT_SEC_DEBUG_EN, class: "fuse" },
    Reg { name: "fuse_opt_wpr_enabled", off: FUSE_OPT_WPR_ENABLED, class: "fuse" },
    Reg { name: "fuse_opt_vpr_enabled", off: FUSE_OPT_VPR_ENABLED, class: "fuse" },
    Reg { name: "mc_enable", off: MC_ENABLE, class: "mc" },
    Reg { name: "mc_elpg_enable", off: MC_ELPG_ENABLE, class: "mc" },
    Reg { name: "top_device_info_cfg", off: TOP_DEVICE_INFO_CFG, class: "top" },
    Reg { name: "top_num_gpcs", off: TOP_NUM_GPCS, class: "top" },
    Reg { name: "gsp_falcon_hwcfg", off: GSP_FALCON_BASE + FALCON_HWCFG_OFF, class: "gsp-falcon-v1" },
    Reg { name: "gsp_falcon_dmactl", off: GSP_FALCON_BASE + FALCON_DMACTL_OFF, class: "gsp-falcon-v1" },
    Reg { name: "gsp_falcon_idlestate", off: GSP_FALCON_BASE + FALCON_IDLESTATE_OFF, class: "gsp-falcon-v1" },
    Reg { name: "gsp_falcon_irqmask", off: GSP_FALCON_BASE + FALCON_IRQMASK_OFF, class: "gsp-falcon-v1" },
    Reg { name: "gsp_falcon_irqdest", off: GSP_FALCON_BASE + FALCON_IRQDEST_OFF, class: "gsp-falcon-v1" },
    Reg { name: "gsp_falcon_cpuctl_v1", off: GSP_FALCON_BASE + FALCON_CPUCTL_OFF, class: "gsp-falcon-v1" },
    Reg { name: "gsp_falcon_hwcfg2", off: GSP_FALCON_BASE + FALCON_HWCFG2_OFF, class: "gsp-falcon-v1" },
    Reg { name: "gsp_falcon_mailbox0", off: GSP_FALCON_BASE + FALCON_MAILBOX0_OFF, class: "gsp-falcon-v1" },
    Reg { name: "priscv_bcr_ctrl", off: GSP_FALCON2_BASE + PRISCV_BCR_CTRL_OFF, class: "gsp-priscv-bcr" },
    Reg { name: "priscv_bcr_dmacfg", off: GSP_FALCON2_BASE + PRISCV_BCR_DMACFG_OFF, class: "gsp-priscv-bcr" },
    Reg { name: "priscv_bcr_pkcparam_lo", off: GSP_FALCON2_BASE + PRISCV_BCR_PKCPARAM_LO_OFF, class: "gsp-priscv-bcr" },
    Reg { name: "priscv_bcr_pkcparam_hi", off: GSP_FALCON2_BASE + PRISCV_BCR_PKCPARAM_HI_OFF, class: "gsp-priscv-bcr" },
    Reg { name: "priscv_bcr_fmccode_lo", off: GSP_FALCON2_BASE + PRISCV_BCR_FMCCODE_LO_OFF, class: "gsp-priscv-bcr" },
    Reg { name: "priscv_bcr_fmccode_hi", off: GSP_FALCON2_BASE + PRISCV_BCR_FMCCODE_HI_OFF, class: "gsp-priscv-bcr" },
    Reg { name: "priscv_bcr_fmcdata_lo", off: GSP_FALCON2_BASE + PRISCV_BCR_FMCDATA_LO_OFF, class: "gsp-priscv-bcr" },
    Reg { name: "priscv_bcr_fmcdata_hi", off: GSP_FALCON2_BASE + PRISCV_BCR_FMCDATA_HI_OFF, class: "gsp-priscv-bcr" },
    Reg { name: "priscv_boot_vector_lo", off: GSP_FALCON2_BASE + PRISCV_BOOT_VECTOR_LO_OFF, class: "gsp-priscv-bcr" },
    Reg { name: "priscv_boot_vector_hi", off: GSP_FALCON2_BASE + PRISCV_BOOT_VECTOR_HI_OFF, class: "gsp-priscv-bcr" },
    Reg { name: "priscv_riscv_irqmask", off: GSP_FALCON2_BASE + PRISCV_RISCV_IRQMASK_OFF, class: "gsp-priscv-bcr" },
    Reg { name: "priscv_riscv_irqdest", off: GSP_FALCON2_BASE + PRISCV_RISCV_IRQDEST_OFF, class: "gsp-priscv-bcr" },
    Reg { name: "priscv_cpuctl", off: GSP_FALCON2_BASE + PRISCV_CPUCTL_OFF, class: "gsp-priscv-bcr" },
    Reg { name: "priscv_br_retcode", off: GSP_FALCON2_BASE + PRISCV_BR_RETCODE_OFF, class: "gsp-priscv-bcr" },
    Reg { name: "pmu_falcon2_cpuctl", off: PMU_FALCON2_BASE + PRISCV_CPUCTL_OFF, class: "pmu-falcon2" },
];

/// Milliseconds on CNTPCT (rung 4c's helper, same arithmetic).
fn now_ms() -> u64 {
    let freq: u64;
    let now: u64;
    unsafe {
        core::arch::asm!("mrs {}, CNTFRQ_EL0", out(reg) freq, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, CNTPCT_EL0", out(reg) now, options(nomem, nostack, preserves_flags));
    }
    now / (freq / 1000).max(1)
}

/// One announced read, result printed by the caller.
fn read5(phase: &str, name: &str, addr: u64) -> u32 {
    serial_println!("[{}] about-to-read {} {} reg={:#x} — if this is the LAST line, THAT read was EL3-fatal and the boot ended inside it", FAM, phase, name, addr);
    r32(addr)
}

/// The 30-register census: pass 1 stores, pass 2 diffs against pass 1 and judges the BCR eight against
/// what the ignition WROTE. Returns (readable, unreadable, changed_ex_bcr_retcode, became_readable,
/// became_unreadable, bcr_intact, bcr_altered, bcr_unread).
fn census(post: bool, base: u64, pre: &mut [u32; R5C_N], bcr_expect: &[u32; 8]) -> (u32, u32, u32, u32, u32, u32, u32, u32) {
    let phase = if post { "post-ignition" } else { "pre-ignition" };
    let mut cur_class = "";
    let (mut n_read, mut n_unread, mut changed_ex, mut became_r, mut became_u, mut bi, mut ba, mut bu) = (0u32, 0u32, 0u32, 0u32, 0u32, 0u32, 0u32, 0u32);
    for (i, r) in CENSUS.iter().enumerate() {
        if r.class != cur_class {
            cur_class = r.class;
            serial_println!("[{}] {} address class {} (KNOWN — read on this die by rungs 1-4c without fault; BAR0={:#x})", FAM, phase, r.class, base);
        }
        let v = read5(phase, r.name, base + r.off);
        let unread = unreadable_reason(v);
        if unread.is_some() { n_unread += 1; } else { n_read += 1; }
        if !post {
            pre[i] = v;
            match unread {
                Some(why) => serial_println!("[{}] pre-ignition {} @{:#x} = -UNREADABLE reason={} val={:#010x}", FAM, r.name, r.off, why, v),
                None => serial_println!("[{}] pre-ignition {} @{:#x} = {:#010x}", FAM, r.name, r.off, v),
            }
            continue;
        }
        let p = pre[i];
        let p_unread = unreadable_reason(p).is_some();
        let is_bcr = (R5C_BCR_FIRST..=R5C_BCR_LAST).contains(&i);
        let diff = if is_bcr {
            "written-by-5a"
        } else if p_unread && unread.is_none() {
            became_r += 1;
            "became-readable"
        } else if !p_unread && unread.is_some() {
            became_u += 1;
            "became-unreadable"
        } else if p != v {
            if i != R5C_BR_RETCODE { changed_ex += 1; }
            "changed"
        } else {
            "same"
        };
        let mut note = "";
        if is_bcr {
            let e = bcr_expect[i - R5C_BCR_FIRST];
            if unread.is_some() { bu += 1; note = " intact=-UNREADABLE"; } else if v == e { bi += 1; note = " intact=1"; } else { ba += 1; note = " intact=0"; }
        }
        match unread {
            Some(why) => serial_println!("[{}] post-ignition {} @{:#x} = -UNREADABLE reason={} val={:#010x} diff={} pre={:#010x}{}", FAM, r.name, r.off, why, v, diff, p, note),
            None => serial_println!("[{}] post-ignition {} @{:#x} = {:#010x} diff={} pre={:#010x}{}", FAM, r.name, r.off, v, diff, p, note),
        }
        if is_bcr {
            serial_println!("[{}] post-ignition {} expected={:#010x}{}", FAM, r.name, bcr_expect[i - R5C_BCR_FIRST], note);
        }
        let _ = R5C_MAILBOX0;
    }
    (n_read, n_unread, changed_ex, became_r, became_u, bi, ba, bu)
}

/// The three post-ignition reads rung 4b makes, tagged `post-ignition ` in THIS family. Returns
/// (halted, lockdown, v1_readable, mailbox0).
fn post_block(base: u64, f2: u64) -> (u32, u32, u32, u32) {
    let c = read5("post-ignition", "priscv_cpuctl", f2 + PRISCV_CPUCTL_OFF);
    let halted = match unreadable_reason(c) { Some(_) => 1, None => (c >> PRISCV_CPUCTL_HALTED_BIT) & 1 };
    serial_println!("[{}] post-ignition priscv_cpuctl halted={} (raw={:#010x})", FAM, halted, c);
    let h = read5("post-ignition", "falcon_hwcfg2", base + GSP_FALCON_BASE + FALCON_HWCFG2_OFF);
    let lockdown = match unreadable_reason(h) { Some(_) => 1, None => (h >> HWCFG2_PRIV_LOCKDOWN_BIT) & 1 };
    serial_println!("[{}] post-ignition hwcfg2 lockdown={} (raw={:#010x}) -> {}", FAM, lockdown, h, if lockdown == 0 { "POSTLOCK-DROPPED" } else { "POSTLOCK-HELD" });
    let v = read5("post-ignition", "gsp_falcon_cpuctl_v1", base + GSP_FALCON_BASE + FALCON_CPUCTL_OFF);
    let v1r = unreadable_reason(v).is_none() as u32;
    serial_println!("[{}] post-ignition gsp_falcon_cpuctl_v1 readable={} (raw={:#010x}) -> {}", FAM, v1r, v, if v1r == 1 { "V1MIRROR-READABLE" } else { "V1MIRROR-CLOSED" });
    let m = read5("post-ignition", "gsp_falcon_mailbox0", base + GSP_FALCON_BASE + FALCON_MAILBOX0_OFF);
    (halted, lockdown, v1r, m)
}

/// First four words at a placement — a CPU read of this kernel's own window, never a register.
fn words4(pa: u64) -> [u32; 4] {
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
    [r32(pa), r32(pa + 4), r32(pa + 8), r32(pa + 12)]
}

/// SHA-256 of a placed section as it sits in the window now.
fn digest_at(p: &Placed) -> [u8; 32] {
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
    let s = unsafe { core::slice::from_raw_parts(p.pa as *const u8, p.size as usize) };
    let mut h = crate::hash::Sha256::new();
    h.update(s);
    h.finalize()
}

/// RUNG 5a — the entry, one folded call on the `sdmmc_census` line of `tegra_early_stop` (after the card
/// is published; the loader reads it). RETURNS on every zero-MMIO refusal; ends in SYSTEM_OFF once the lock
/// is written.
pub fn ga10bprobe5_run(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) {
    serial_println!("[ga10bprobe5a] rung 5a ARMED (UNAOS_GA10B_PROBE5={}) — the VENDOR IGNITION: rung 4b's seven writes with ONLY THE PAYLOAD changed. Order: zero-MMIO phase (window, pattern, the three vendor files read from /boot/GA10B/ through the VFS, sized, placed 256-aligned, digested in the window — any refusal RETURNS with zero MMIO) -> rung 4a's bracket and BCR census re-proven -> rung 4c's pre-ignition census -> addresses at the three placements (form={} shift={}) -> bcr_dmacfg noncoherent|lock (SPENDS THE POWER CYCLE) -> bcr_ctrl 0x111 -> priscv_cpuctl startcpu -> bounded br_retcode poll -> the oracles -> verdict -> SYSTEM_OFF. Verdict vocabulary: ACR-ACCEPTED | BROM-VERDICT-FAIL code=<retcode> | BROM-VERDICT-PASS-UNWITNESSED code=<retcode> | BROM-NOVERDICT | IGNITION-SKIPPED reason=<bcr-not-allheld|bcr-addr-refused> | BCR-CTRL-REFUSED | REFUSED reason=<image-absent|image-size|image-digest|image-window|image-unaligned|no-dma-window|no-gpu-node|no-power-domains|pg-timeout|pg-on-refused|pg-readback-not-on|bcr-locked|bcr-dmacfg-unreadable|bcr-ctrl-unreadable>. F21 warning: a fabric RAS from a real image whose DMA reach we do not bound may need a manual power cut", if ADDR_SHIFT == 8 { "5" } else { "1" }, ADDR_FORM, ADDR_SHIFT);
    serial_println!("[ga10bprobe5a] addr_encoding={} from={} (P5x: rung 4e, UNAOS_GA10B_PROBE4=5, is the flight that settles the encoding; until its verdict is in docs/dev/evidence/ this build carries the form its knob value named and says so here)", ADDR_FORM, if ADDR_SHIFT == 8 { "unflown(4e; rung-5 brief §1.2 Hopper MIT source)" } else { "flown(4b raw; A51/A55)" });

    // ── 0. ZERO-MMIO PHASE ─────────────────────────────────────────────────────────────────────────────
    let (wb, ws) = super::mmu_tegra::ga10b4_nc_window();
    if wb == 0 || ws == 0 {
        serial_println!("[ga10bprobe5a] -> REFUSED reason=no-dma-window — no rung-4 2 MiB block was seated below 4 GiB (see the [ga10b4nc] census at heap-guard); zero MMIO; RETURNING");
        return;
    }
    if wb + ws > 0x1_0000_0000 || !super::mmu_tegra::install_nc_window(wb, ws) {
        serial_println!("[ga10bprobe5a] -> REFUSED reason=no-dma-window — the seated block could not be mapped Normal-NC (or is not below 4 GiB); zero MMIO; RETURNING");
        return;
    }
    {
        let mut off = 0u64;
        while off < ws {
            unsafe { core::ptr::write_volatile((wb + off) as *mut u32, DMABUF_PATTERN) };
            off += 4;
        }
        unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
    }
    serial_println!("[ga10bprobe5a] dmabuf_pa={:#010x} dmabuf_size={:#x} dmabuf_pattern={:#010x} filled {} KiB, dsb sy (this kernel's OWN block, Normal-NC, below 4 GiB; the unwritten tail stays the pattern)", wb, ws, DMABUF_PATTERN, ws >> 10);
    let mt = crate::shell::vfs_mount_table();
    let placed = match ga10b_fw::load(&mt, "/boot/GA10B", &ACR_GSP, wb, ws) {
        Outcome::Loaded { placed, total, window, end } => {
            serial_println!("[ga10bprobe5a] image loaded: total={} window={:#x}..{:#x} image_sections=3 image_digests_ok=3", total, window, end);
            placed
        }
        Outcome::Refused { reason, name } => {
            serial_println!("[ga10bprobe5a] -> REFUSED reason=image-{} name={} — a media fault, not a GPU one; zero MMIO; the boot RETURNS and continues", reason, name);
            return;
        }
    };
    // P7 — the placements, one line per section, with the register value each becomes.
    let (vals, raws) = addr_values(&placed);
    let mut unaligned = false;
    for i in 0..3 {
        let p = &placed[i];
        serial_println!("[ga10bprobe5a] section={} file={} size={} off={:#x} pa={:#010x} reg_lo={:#010x} reg_hi={:#010x} form={} aligned256={}", ROLE[i], ACR_GSP[i].name, p.size, p.off, p.pa, vals[2 * i], vals[2 * i + 1], ADDR_FORM, (p.pa & 0xff == 0) as u32);
        if p.pa & 0xff != 0 { unaligned = true; }
    }
    if unaligned {
        serial_println!("[ga10bprobe5a] -> REFUSED reason=image-unaligned — a placement is not 256-byte aligned; zero MMIO; RETURNING");
        return;
    }
    let pre_words = [words4(placed[0].pa), words4(placed[1].pa), words4(placed[2].pa)];
    for i in 0..3 {
        serial_println!("[ga10bprobe5a] pre-ignition dmabuf {} @{:#x} = {:#010x} {:#010x} {:#010x} {:#010x} (first 4 words of the placed section, CPU side)", ROLE[i], placed[i].pa, pre_words[i][0], pre_words[i][1], pre_words[i][2], pre_words[i][3]);
    }

    // ── 1. P0 — the bracket (rung 4a's, in this family) ───────────────────────────────────────────────
    let Some(gpu) = resolve_gpu_node(dtb_addr, dtb_size, ram_gib_mask) else {
        serial_println!("[ga10bprobe5a] -> REFUSED reason=no-gpu-node — the firmware DTB carries no usable gpu@ node; zero MMIO; RETURNING");
        return;
    };
    serial_println!("[ga10bprobe5a] gpu@ node: BAR0={:#x} power-domain-id={} clocks={}: {} {} {} {} {} {} {} {}", gpu.bar0, match gpu.pd_id { Some(id) => id as i64, None => -1 }, gpu.n_clocks, gpu.clocks[0], gpu.clocks[1], gpu.clocks[2], gpu.clocks[3], gpu.clocks[4], gpu.clocks[5], gpu.clocks[6], gpu.clocks[7]);
    let Some(pd_id) = gpu.pd_id else {
        serial_println!("[ga10bprobe5a] -> REFUSED reason=no-power-domains — gpu@ lists no power-domains id; a gated access is EL3-fatal (JX1); zero MMIO; RETURNING");
        return;
    };
    let Some(g) = super::fdt_tegra::bpmp_geometry(dtb_addr, dtb_size, ram_gib_mask) else {
        serial_println!("[ga10bprobe5a] -> REFUSED reason=pg-timeout — no BPMP geometry in the DTB; zero MMIO; RETURNING");
        return;
    };
    let Some(chan) = super::bpmp_tegra::chan_reopen(&g) else {
        serial_println!("[ga10bprobe5a] -> REFUSED reason=pg-timeout — the BPMP channel could not be reopened from the DTB geometry; zero MMIO; RETURNING");
        return;
    };
    let chan = &chan;
    serial_println!("[ga10bprobe5a] BPMP MRQ_PG GET_STATE (read-only) id={} — the pre-state, before anything is driven", pd_id);
    let pg_before = match pg_state(chan, pd_id) {
        Some((err, st)) => {
            serial_println!("[ga10bprobe5a] pg-before id={} err={} state={:#x}", pd_id, err, st);
            if err == 0 { Some(st) } else { None }
        }
        None => {
            serial_println!("[ga10bprobe5a] -> REFUSED reason=pg-timeout — MRQ_PG GET_STATE got no frame in 100 ms; zero MMIO; RETURNING");
            return;
        }
    };
    let mut we_powered = false;
    if pg_before != Some(PG_STATE_ON) {
        serial_println!("[ga10bprobe5a] BPMP MRQ_PG SET_STATE id={} state=ON — a BPMP request, not an MMIO write", pd_id);
        match chan.transfer(MRQ_PG, &[CMD_PG_SET_STATE, pd_id, PG_STATE_ON]) {
            Some((err, _)) => {
                serial_println!("[ga10bprobe5a] pg-set-on id={} err={}", pd_id, err);
                we_powered = err == 0;
            }
            None => serial_println!("[ga10bprobe5a] pg-set-on id={} TIMEOUT", pd_id),
        }
        settle_ms(2);
    }
    let pg_now = match pg_state(chan, pd_id) {
        Some((err, st)) => {
            serial_println!("[ga10bprobe5a] pg-readback id={} err={} state={:#x}", pd_id, err, st);
            if err == 0 { st } else { 0xffff_ffff }
        }
        None => {
            serial_println!("[ga10bprobe5a] pg-readback id={} TIMEOUT", pd_id);
            0xffff_ffff
        }
    };
    let mut enabled_by_us = [false; 8];
    let mut n_on_after = 0usize;
    for i in 0..gpu.n_clocks {
        let id = gpu.clocks[i];
        let before = match clk(chan, CMD_CLK_IS_ENABLED, id) {
            Some((err, st)) => {
                serial_println!("[ga10bprobe5a] clk {} IS_ENABLED (before) err={} = {}", id, err, st);
                if err == 0 { Some(st) } else { None }
            }
            None => {
                serial_println!("[ga10bprobe5a] clk {} IS_ENABLED (before) TIMEOUT", id);
                None
            }
        };
        if before == Some(0) {
            serial_println!("[ga10bprobe5a] clk {} ENABLE — BPMP request; if this is the LAST line the transaction hung the boot", id);
            match clk(chan, CMD_CLK_ENABLE, id) {
                Some((err, _)) => {
                    serial_println!("[ga10bprobe5a] clk {} ENABLE err={}", id, err);
                    enabled_by_us[i] = err == 0;
                }
                None => serial_println!("[ga10bprobe5a] clk {} ENABLE TIMEOUT", id),
            }
        }
    }
    for i in 0..gpu.n_clocks {
        let id = gpu.clocks[i];
        match clk(chan, CMD_CLK_IS_ENABLED, id) {
            Some((err, st)) => {
                serial_println!("[ga10bprobe5a] clk {} IS_ENABLED (after) err={} = {}", id, err, st);
                if err == 0 && st == 1 { n_on_after += 1; }
            }
            None => serial_println!("[ga10bprobe5a] clk {} IS_ENABLED (after) TIMEOUT", id),
        }
    }
    serial_println!("[ga10bprobe5a] clocks: {} of {} running after this rung's enables (236 answering err=-22 is the rung-3 datum, expected)", n_on_after, gpu.n_clocks);
    settle_ms(2);

    // The symmetric restore of the bracket, used by every RETURNING path below the power-on.
    let restore = |chan: &super::bpmp_tegra::Chan| {
        let mut n_disabled = 0usize;
        for i in (0..gpu.n_clocks).rev() {
            if enabled_by_us[i] {
                let id = gpu.clocks[i];
                match clk(chan, CMD_CLK_DISABLE, id) {
                    Some((err, _)) => {
                        serial_println!("[ga10bprobe5a] clk {} DISABLE (restore) err={}", id, err);
                        if err == 0 { n_disabled += 1; }
                    }
                    None => serial_println!("[ga10bprobe5a] clk {} DISABLE (restore) TIMEOUT", id),
                }
            }
        }
        let mut pg_final = pg_now;
        if we_powered {
            match chan.transfer(MRQ_PG, &[CMD_PG_SET_STATE, pd_id, PG_STATE_OFF]) {
                Some((err, _)) => serial_println!("[ga10bprobe5a] pg-set-off (restore) id={} err={}", pd_id, err),
                None => serial_println!("[ga10bprobe5a] pg-set-off (restore) id={} TIMEOUT", pd_id),
            }
            match pg_state(chan, pd_id) {
                Some((err, st)) => {
                    serial_println!("[ga10bprobe5a] pg-final id={} err={} state={:#x}", pd_id, err, st);
                    pg_final = st;
                }
                None => serial_println!("[ga10bprobe5a] pg-final id={} TIMEOUT", pd_id),
            }
        }
        serial_println!("[ga10bprobe5a] restored: pg={:#x} (was {}) clocks-disabled={} of {} enabled here", pg_final, match pg_before { Some(s) => s as i64, None => -1 }, n_disabled, enabled_by_us.iter().filter(|b| **b).count());
    };

    if pg_now != PG_STATE_ON {
        serial_println!("[ga10bprobe5a] -> REFUSED reason={} — the explicit readback did not say ON; a gated access is EL3-fatal (JX1): NOT ONE BAR0 register was touched; RETURNING", if we_powered { "pg-readback-not-on" } else { "pg-on-refused" });
        restore(chan);
        return;
    }
    let base = gpu.bar0;
    let f2 = base + GSP_FALCON2_BASE;

    // ── P1 / P2 ────────────────────────────────────────────────────────────────────────────────────────
    let a = f2 + PRISCV_BCR_DMACFG_OFF;
    serial_println!("[ga10bprobe5a] about-to-read priscv_bcr_dmacfg reg={:#x} (P1: lock_locked bit31 must be 0) — if this is the LAST line, THAT read was EL3-fatal", a);
    let v = r32(a);
    if let Some(r) = unreadable_reason(v) {
        serial_println!("[ga10bprobe5a] priscv_bcr_dmacfg @{:#x} = -UNREADABLE reason={} val={:#010x}", PRISCV_BCR_DMACFG_OFF, r, v);
        serial_println!("[ga10bprobe5a] -> REFUSED reason=bcr-dmacfg-unreadable — zero writes; RETURNING");
        restore(chan);
        return;
    }
    serial_println!("[ga10bprobe5a] priscv_bcr_dmacfg @{:#x} = {:#010x} lock_locked={}", PRISCV_BCR_DMACFG_OFF, v, (v & BCR_DMACFG_LOCK_LOCKED != 0) as u32);
    if v & BCR_DMACFG_LOCK_LOCKED != 0 {
        serial_println!("[ga10bprobe5a] -> REFUSED reason=bcr-locked — the BCR is spent for this power cycle; zero writes; the next boot must be COLD; RETURNING");
        restore(chan);
        return;
    }
    let a = f2 + PRISCV_BCR_CTRL_OFF;
    serial_println!("[ga10bprobe5a] about-to-read priscv_bcr_ctrl reg={:#x} (P2: baseline; rung 3 read 0x00000110) — if this is the LAST line, THAT read was EL3-fatal", a);
    let ctrl_baseline = r32(a);
    if let Some(r) = unreadable_reason(ctrl_baseline) {
        serial_println!("[ga10bprobe5a] priscv_bcr_ctrl @{:#x} = -UNREADABLE reason={} val={:#010x}", PRISCV_BCR_CTRL_OFF, r, ctrl_baseline);
        serial_println!("[ga10bprobe5a] -> REFUSED reason=bcr-ctrl-unreadable — zero writes; RETURNING");
        restore(chan);
        return;
    }
    serial_println!("[ga10bprobe5a] bcr_ctrl_before={:#010x} bcr_ctrl_baseline_changed={} (a value other than 0x00000110 is a datum, not a stop)", ctrl_baseline, (ctrl_baseline != 0x0000_0110) as u32);

    // ── P5 — rung 4a's census, re-run in this family: seven writes at the PLACED addresses, restore ───
    let mut held = 0u32;
    let mut n_unreadable = 0u32;
    let mut written = [false; 7];
    let mut stopped = false;
    for i in 0..6 {
        let (name, off) = BCR_ADDR_REGS[i];
        written[i] = true;
        match addr_write_verify(name, f2, off, vals[i], raws[i], "BCR DMA address, P5 census A-step") {
            Ok(true) => held += 1,
            Ok(false) => { stopped = true; }
            Err(_) => { n_unreadable += 1; stopped = true; }
        }
        if stopped {
            serial_println!("[ga10bprobe5a] write list STOPPED at {} — the remaining address writes are not attempted (F3/F4)", name);
            break;
        }
    }
    let mut selflocked = false;
    if !stopped {
        written[6] = true;
        match bcr_write_verify(FAM, "priscv_bcr_dmacfg", f2, PRISCV_BCR_DMACFG_OFF, BCR_DMACFG_TARGET_NONCOHERENT_SYSTEM, "target_noncoherent_system ONLY; the lock_locked bit is DELIBERATELY NOT SET") {
            Ok(true) => held += 1,
            Ok(false) => {
                let got = r32(f2 + PRISCV_BCR_DMACFG_OFF);
                if got & BCR_DMACFG_LOCK_LOCKED != 0 {
                    selflocked = true;
                    serial_println!("[ga10bprobe5a] dmacfg readback {:#010x} has lock_locked SET without being asked (F6)", got);
                }
            }
            Err(_) => { n_unreadable += 1; }
        }
    }
    let mut sticky: Option<&str> = None;
    for i in 0..7 {
        if !written[i] { continue; }
        let (name, off) = if i < 6 { BCR_ADDR_REGS[i] } else { ("priscv_bcr_dmacfg", PRISCV_BCR_DMACFG_OFF) };
        match bcr_write_verify(FAM, name, f2, off, 0, "restore to zero, as found") {
            Ok(true) => {}
            Ok(false) => { if sticky.is_none() { sticky = Some(name); } }
            Err(_) => { n_unreadable += 1; }
        }
    }
    let a = f2 + PRISCV_BCR_DMACFG_OFF;
    serial_println!("[ga10bprobe5a] about-to-read priscv_bcr_dmacfg reg={:#x} (lock_after) — if this is the LAST line, THAT read was EL3-fatal", a);
    let after = r32(a);
    let lock_after = (unreadable_reason(after).is_none() && after & BCR_DMACFG_LOCK_LOCKED != 0) as u32;
    let arm = if selflocked || lock_after == 1 { "BCR-SELFLOCKED" } else if sticky.is_some() { "BCR-STICKY" } else if held == 7 { "BCR-ALLHELD" } else if held == 0 { "BCR-NONEHELD" } else { "BCR-SOMEHELD" };
    serial_println!("[ga10bprobe5a] bcrheld={}/7 dmabuf_pa={:#010x} form={} lock_after={} unreadable={} -> {}", held, wb, ADDR_FORM, lock_after, n_unreadable, arm);
    if arm == "BCR-SELFLOCKED" || arm == "BCR-STICKY" {
        serial_println!("[ga10bprobe5a] the census did not leave the BCR as found ({}): the power cycle is spent and the next boot must be COLD (F6/F7); no ignition", arm);
        finish4(FAM);
    }
    if arm != "BCR-ALLHELD" {
        serial_println!("[ga10bprobe5a] br_retcode=0x00000000 br_result=0x0 samples=0/{} lock_latched=0 post_lockdown=0 v1_readable=0 form={} -> IGNITION-SKIPPED reason=bcr-not-allheld — no lock, no trigger, no ignition; nothing was spent; RETURNING", BR_POLL_SAMPLES, ADDR_FORM);
        restore(chan);
        return;
    }

    // ── 3. PRE-ignition census ─────────────────────────────────────────────────────────────────────────
    let mut pre = [0u32; R5C_N];
    serial_println!("[ga10bprobe5a] pass 1 — PRE-ignition census of {} registers (after the census restore, before the ignition; read-only)", R5C_N);
    let (nr, nu, _, _, _, _, _, _) = census(false, base, &mut pre, &[0u32; 8]);
    serial_println!("[ga10bprobe5a] pre-ignition census: readable={}/{} unreadable={} -> PRECENSUS-DONE", nr, R5C_N, nu);

    // ── 4. THE IGNITION ────────────────────────────────────────────────────────────────────────────────
    serial_println!("[ga10bprobe5a] the IGNITION — B0 the six addresses at the placed sections, B1 the lock, B2 the trigger, B4 startcpu, B5 the bounded poll ({} samples, {} ms apart)", BR_POLL_SAMPLES, BR_POLL_SETTLE_MS);
    for i in 0..6 {
        let (name, off) = BCR_ADDR_REGS[i];
        let ok = matches!(addr_write_verify(name, f2, off, vals[i], raws[i], "BCR DMA address, B0 re-write at the placed section"), Ok(true));
        if !ok {
            serial_println!("[ga10bprobe5a] B0 mismatch at {} — restoring the addresses to zero and skipping the ignition", name);
            for j in 0..=i {
                let (nm, of) = BCR_ADDR_REGS[j];
                let _ = bcr_write_verify(FAM, nm, f2, of, 0, "restore to zero after B0 mismatch");
            }
            serial_println!("[ga10bprobe5a] br_retcode=0x00000000 br_result=0x0 samples=0/{} lock_latched=0 post_lockdown=0 v1_readable=0 form={} -> IGNITION-SKIPPED reason=bcr-addr-refused", BR_POLL_SAMPLES, ADDR_FORM);
            finish4(FAM);
        }
    }
    let a = f2 + PRISCV_BCR_DMACFG_OFF;
    let lockval = BCR_DMACFG_TARGET_NONCOHERENT_SYSTEM | BCR_DMACFG_LOCK_LOCKED;
    serial_println!("[ga10bprobe5a] about-to-WRITE priscv_bcr_dmacfg reg={:#x} val={:#010x} (target_noncoherent_system | lock_locked — THIS SPENDS THE POWER CYCLE: the BCR cannot be reprogrammed again until a cold boot) — if this is the LAST line, THAT WRITE was fatal and the boot ended inside it", a, lockval);
    ignite5_w32(a, lockval);
    let got = r32(a);
    let lock_latched = (unreadable_reason(got).is_none() && got & BCR_DMACFG_LOCK_LOCKED != 0) as u32;
    serial_println!("[ga10bprobe5a] priscv_bcr_dmacfg @{:#x} wrote={:#010x} read={:#010x} lock_latched={} ({})", PRISCV_BCR_DMACFG_OFF, lockval, got, lock_latched, if lock_latched == 1 { "the lock took" } else { "the lock did NOT latch — a datum; continuing" });
    let a = f2 + PRISCV_BCR_CTRL_OFF;
    serial_println!("[ga10bprobe5a] about-to-WRITE priscv_bcr_ctrl reg={:#x} val={:#010x} (the ACKED SEQ brom_config value; baseline was {:#010x}) — if this is the LAST line, THAT WRITE was fatal and the boot ended inside it", a, BCR_CTRL_BROM_CONFIG, ctrl_baseline);
    ignite5_w32(a, BCR_CTRL_BROM_CONFIG);
    let ctrl = r32(a);
    serial_println!("[ga10bprobe5a] priscv_bcr_ctrl @{:#x} wrote={:#010x} read={:#010x} held={}", PRISCV_BCR_CTRL_OFF, BCR_CTRL_BROM_CONFIG, ctrl, (ctrl == BCR_CTRL_BROM_CONFIG) as u32);
    if ctrl != BCR_CTRL_BROM_CONFIG {
        let (halted, lockdown, v1r, _) = post_block(base, f2);
        serial_println!("[ga10bprobe5a] br_retcode=0x00000000 br_result=0x0 samples=0/{} lock_latched={} post_lockdown={} v1_readable={} form={} -> BCR-CTRL-REFUSED read={:#010x} — the trigger register did not take brom_config; the ignition write was NOT issued (halted={})", BR_POLL_SAMPLES, lock_latched, lockdown, v1r, ADDR_FORM, ctrl, halted);
        finish4(FAM);
    }
    serial_println!("[ga10bprobe5a] SEQ step 3 (priscv_boot_vector lo/hi) DELIBERATELY OMITTED — unreadable on this die (0xbadf5620), an unverifiable mutation; the SEQ marks it optional and we take the option, exactly as 4b did");
    let a = f2 + PRISCV_CPUCTL_OFF;
    serial_println!("[ga10bprobe5a] about-to-WRITE priscv_cpuctl reg={:#x} val={:#010x} (startcpu_true) — THE IGNITION, with NVIDIA's signed ACR image behind the descriptor. if this is the LAST line, THAT WRITE was fatal and the boot ended inside it", a, PRISCV_CPUCTL_STARTCPU);
    ignite5_w32(a, PRISCV_CPUCTL_STARTCPU);
    let c0 = r32(a);
    serial_println!("[ga10bprobe5a] priscv_cpuctl @{:#x} wrote={:#010x} read={:#010x} (STARTCPU is write-only: the read is the immediate post-state, not a readback)", PRISCV_CPUCTL_OFF, PRISCV_CPUCTL_STARTCPU, c0);
    let rc = f2 + PRISCV_BR_RETCODE_OFF;
    let mut retcode: u32 = 0;
    let mut samples: u32 = 0;
    for i in 1..=BR_POLL_SAMPLES {
        samples = i;
        serial_println!("[ga10bprobe5a] about-to-read priscv_br_retcode reg={:#x} sample={}/{} — if this is the LAST line, THAT read was EL3-fatal", rc, i, BR_POLL_SAMPLES);
        retcode = r32(rc);
        serial_println!("[ga10bprobe5a] br_retcode={:#010x} br_result={:#x} sample={}/{}", retcode, retcode & 0x3, i, BR_POLL_SAMPLES);
        if unreadable_reason(retcode).is_none() && (retcode & 0x3 == BR_RETCODE_FAIL || retcode & 0x3 == BR_RETCODE_PASS) {
            break;
        }
        settle_ms(BR_POLL_SETTLE_MS);
    }

    // ── 5. THE ORACLES ─────────────────────────────────────────────────────────────────────────────────
    let (halted, lockdown, v1r, mbox) = post_block(base, f2);
    let mbox_meaning = match unreadable_reason(mbox) { Some(_) => "unreadable", None => if mbox == 0 { "unchanged" } else { "fmc-error" } };
    serial_println!("[ga10bprobe5a] mbox-arg-written=none mbox-post-read={:#010x} mbox-post-meaning={} (no argument address was written to MAILBOX0/1 in 5a — the boot-params channel is Hopper-documented and GA10B-unverified, brief §1.4; a non-zero low byte here is the FMC's own error code, F23, the best failure on the list)", mbox, mbox_meaning);
    // The series.
    serial_println!("[ga10bprobe5a] about-to-read series priscv_br_retcode reg={:#x} samples={} settle_ms={} — if this is the LAST line, THAT read was EL3-fatal", rc, BR_SERIES_SAMPLES, BR_SERIES_SETTLE_MS);
    let t0 = now_ms();
    let (mut have, mut last, mut distinct, mut first_v, mut first_i, mut last_i, mut any_unread, mut reason_or) = (false, 0u32, 0u32, 0u32, 0u32, 0u32, false, 0u32);
    for i in 1..=BR_SERIES_SAMPLES {
        let v = r32(rc);
        let t = now_ms().wrapping_sub(t0);
        if unreadable_reason(v).is_some() { any_unread = true; } else { reason_or |= v >> 2; }
        if !have || v != last {
            distinct += 1;
            if !have { first_v = v; first_i = i; }
            last_i = i;
            serial_println!("[ga10bprobe5a] series sample={}/{} t_ms={} br_retcode={:#010x} br_result={:#x} reason_bits={:#010x} (distinct #{})", i, BR_SERIES_SAMPLES, t, v, v & 0x3, v >> 2, distinct);
        }
        have = true;
        last = v;
        if i < BR_SERIES_SAMPLES { settle_ms(BR_SERIES_SETTLE_MS); }
    }
    let series_arm = if any_unread { "BRSERIES-UNREADABLE" } else if distinct == 1 { "BRSERIES-STABLE" } else { "BRSERIES-CHANGED" };
    serial_println!("[ga10bprobe5a] series priscv_br_retcode @{:#x} = distinct={} first={:#010x}@{} last={:#010x}@{} reason_bits_or={:#010x} samples={} elapsed_ms={} -> {}", PRISCV_BR_RETCODE_OFF, distinct, first_v, first_i, last, last_i, reason_or, BR_SERIES_SAMPLES, now_ms().wrapping_sub(t0), series_arm);
    // POSTBCR + MAPDIFF — the census, pass 2. CENSUS order 15..=22: ctrl, dmacfg, pkc lo/hi, fmccode lo/hi,
    // fmcdata lo/hi; `vals` order: fmccode lo/hi, fmcdata lo/hi, pkcparam lo/hi.
    let expect = [BCR_CTRL_BROM_CONFIG, lockval, vals[4], vals[5], vals[0], vals[1], vals[2], vals[3]];
    serial_println!("[ga10bprobe5a] pass 2 — POST-ignition census of {} registers diffed against pass 1; the BCR eight against what 5a WROTE (read-only)", R5C_N);
    let (nr2, nu2, changed_ex, became_r, became_u, bi, ba, bu) = census(true, base, &mut pre, &expect);
    let bcr_arm = if bu > 0 { "POSTBCR-UNREADABLE" } else if ba == 0 { "POSTBCR-INTACT" } else { "POSTBCR-ALTERED" };
    serial_println!("[ga10bprobe5a] bcr post-ignition: intact={}/8 altered={} unreadable={} -> {}", bi, ba, bu, bcr_arm);
    let map_arm = if became_r == 0 && became_u == 0 && changed_ex == 0 { "MAPDIFF-SAME" } else { "MAPDIFF-CHANGED" };
    serial_println!("[ga10bprobe5a] post-ignition census: readable={}/{} unreadable={} became_readable={} became_unreadable={} changed_ex_bcr_retcode={} -> {}", nr2, R5C_N, nu2, became_r, became_u, changed_ex, map_arm);
    // The window: did anything WRITE our buffer (a READ leaves no trace — this cannot see one), per section.
    let mut sections_intact = 0u32;
    for i in 0..3 {
        let p = &placed[i];
        serial_println!("[ga10bprobe5a] about-to-read post-ignition dmabuf {} pa={:#x} size={} (a CPU read of this kernel's OWN Normal-NC window — DRAM the descriptor pointed at, not a GPU register) — if this is the LAST line, THAT read was fatal", ROLE[i], p.pa, p.size);
        let w = words4(p.pa);
        let d = digest_at(p);
        let intact = d == ACR_GSP[i].sha;
        if intact { sections_intact += 1; }
        serial_println!("[ga10bprobe5a] post-ignition dmabuf {} @{:#x} = {:#010x} {:#010x} {:#010x} {:#010x} words_changed_first4={} sha256_intact={} (pre: {:#010x} {:#010x} {:#010x} {:#010x})", ROLE[i], p.pa, w[0], w[1], w[2], w[3], (0..4).filter(|k| w[*k] != pre_words[i][*k]).count(), intact as u32, pre_words[i][0], pre_words[i][1], pre_words[i][2], pre_words[i][3]);
    }
    // The tail beyond the image must still be the pattern.
    let tail_lo = placed[2].pa + ((placed[2].size as u64 + 255) & !255);
    let mut tail_changed = 0u64;
    let mut off = tail_lo;
    while off < wb + ws {
        if r32(off) != DMABUF_PATTERN { tail_changed += 1; }
        off += 4;
    }
    let dma_arm = if sections_intact == 3 && tail_changed == 0 { "DMABUF-UNTOUCHED" } else { "DMABUF-ALTERED" };
    serial_println!("[ga10bprobe5a] post-ignition dmabuf-scan @{:#x} = sections_intact={}/3 tail_words_changed={}/{} (tail from {:#x}) -> {} (UNTOUCHED says nothing WROTE the window; whether the ROM READ it leaves no trace here — the oracles that say so are the lockdown drop and the v1 mirror)", wb, sections_intact, tail_changed, (wb + ws - tail_lo) / 4, tail_lo, dma_arm);

    // ── 6. THE VERDICT ─────────────────────────────────────────────────────────────────────────────────
    let result = if unreadable_reason(retcode).is_some() { 0xf } else { retcode & 0x3 };
    let accepted = lockdown == 0 || v1r == 1;
    let verdict = if accepted {
        "ACR-ACCEPTED"
    } else if result == BR_RETCODE_FAIL {
        "BROM-VERDICT-FAIL"
    } else if result == BR_RETCODE_PASS {
        "BROM-VERDICT-PASS-UNWITNESSED"
    } else {
        "BROM-NOVERDICT"
    };
    serial_println!("[ga10bprobe5a] verdict br_retcode={:#010x} br_result={:#x} samples={}/{} lock_latched={} post_lockdown={} v1_readable={} halted={} form={} image_sections=3 image_digests_ok=3 -> {} code={:#010x}", retcode, result, samples, BR_POLL_SAMPLES, lock_latched, lockdown, v1r, halted, ADDR_FORM, verdict, retcode);
    match verdict {
        "ACR-ACCEPTED" => serial_println!("[ga10bprobe5a] THE ROM ACCEPTED NVIDIA'S IMAGE: a GPU-side observable moved that no unsigned payload ever moved (lockdown={}, v1_readable={}). The signature wall is behind the ladder; what runs now is the ACR/FMC, and rung 5b's oracles (if armed) say what it did next", lockdown, v1r),
        "BROM-VERDICT-FAIL" => serial_println!("[ga10bprobe5a] F19: a real signed image, digests verified in the window, addresses form={}, and the ROM still says FAIL. Do not iterate images this session: record the identity, then decide between the other address form, the second triple (safety-scheduler) and the mapping inference (§4.1.2) — one boot each", ADDR_FORM),
        "BROM-VERDICT-PASS-UNWITNESSED" => serial_println!("[ga10bprobe5a] F20: the code says PASS and neither oracle moved — a measurement error until re-flown from a cold boot; build nothing on it this session"),
        _ => serial_println!("[ga10bprobe5a] no verdict in {} samples: halted={} — halted=1 means the core never started or halted again; halted=0 means it is RUNNING and the poll was short (F5)", BR_POLL_SAMPLES, halted),
    }
    #[cfg(feature = "ga10bprobe5b")]
    super::ga10b_ignite::rung5b(base, verdict);
    let _ = (series_arm, bcr_arm, map_arm, dma_arm);
    finish4(FAM);
}

/// RUNG 5b — the ACR->PMU handshake, READ-ONLY oracles (`ga10bprobe5b`; design GA10B-RUNG6-BRIEF.md §3;
/// ledger A64). Only after `ACR-ACCEPTED`; otherwise prints why it did not run.
#[cfg(feature = "ga10bprobe5b")]
pub fn rung5b(base: u64, verdict: &str) {
    const F5B: &str = "ga10bprobe5b";
    /// PUBLIC-RECALLED, NOT ACKED: the PMU's legacy falcon aperture base (open-gpu-kernel-modules
    /// `dev_pwr_pri.h`, MIT: NV_PPWR_FALCON_* at 0x10a000 + falcon offset). Rung 3 read the PMU's falcon2
    /// (priscv) block at 0x10b000 on this die without fault; this v1 block is one page below it and has
    /// never been touched here — every read is announced, every value printed raw.
    const PMU_FALCON_BASE: u64 = 0x0010_a000;
    /// PUBLIC-RECALLED (dev_falcon_v4.h, MIT): MAILBOX1 at +0x044, one word above the metal-proven MAILBOX0.
    const FALCON_MAILBOX1_OFF: u64 = 0x044;
    const SAMPLES: u32 = 10;
    const SETTLE_MS: u64 = 20;
    serial_println!("[ga10bprobe5b] rung 5b ARMED (UNAOS_GA10B_PROBE5=2|7) — the ACR->PMU handshake, READ-ONLY: after ACR-ACCEPTED the ACR (FMC) is expected to carve the WPR and boot the PMU (GA10B-RUNG6-BRIEF.md §2, from nvgpu's public sequence); this rung ADDS ZERO WRITES and samples the channels that would show it — GSP mailbox0/1 (the FMC's status/error words), the PMU legacy-falcon block (cpuctl, hwcfg2, mailbox0/1; PUBLIC-RECALLED base 0x10a000, first touch on this die) and the PMU falcon2 cpuctl rung 3 read — {} samples {} ms apart. Vocabulary: PMU-RUNNING | PMU-HALTED | PMU-UNREADABLE | PMU-NOT-ATTEMPTED reason=<not-accepted>", SAMPLES, SETTLE_MS);
    if verdict != "ACR-ACCEPTED" {
        serial_println!("[ga10bprobe5b] -> PMU-NOT-ATTEMPTED reason=not-accepted (5a verdict {}): nothing past the ROM ran, so there is no handshake to observe; zero reads", verdict);
        return;
    }
    let regs: [(&str, u64); 8] = [
        ("gsp_falcon_mailbox0", base + GSP_FALCON_BASE + FALCON_MAILBOX0_OFF),
        ("gsp_falcon_mailbox1", base + GSP_FALCON_BASE + FALCON_MAILBOX1_OFF),
        ("gsp_falcon_hwcfg2", base + GSP_FALCON_BASE + FALCON_HWCFG2_OFF),
        ("pmu_falcon_cpuctl_v1", base + PMU_FALCON_BASE + FALCON_CPUCTL_OFF),
        ("pmu_falcon_hwcfg2", base + PMU_FALCON_BASE + FALCON_HWCFG2_OFF),
        ("pmu_falcon_mailbox0", base + PMU_FALCON_BASE + FALCON_MAILBOX0_OFF),
        ("pmu_falcon_mailbox1", base + PMU_FALCON_BASE + FALCON_MAILBOX1_OFF),
        ("pmu_falcon2_cpuctl", base + PMU_FALCON2_BASE + PRISCV_CPUCTL_OFF),
    ];
    let mut last = [0u32; 8];
    let mut changed = [0u32; 8];
    let mut unread = [0u32; 8];
    for s in 1..=SAMPLES {
        for (k, (name, addr)) in regs.iter().enumerate() {
            serial_println!("[ga10bprobe5b] about-to-read handshake {} reg={:#x} sample={}/{} — if this is the LAST line, THAT read was EL3-fatal and the boot ended inside it", name, addr, s, SAMPLES);
            let v = r32(*addr);
            let u = unreadable_reason(v);
            if u.is_some() { unread[k] += 1; }
            if s > 1 && v != last[k] { changed[k] += 1; }
            match u {
                Some(why) => serial_println!("[ga10bprobe5b] handshake {} @{:#x} = -UNREADABLE reason={} val={:#010x} sample={}", name, addr, why, v, s),
                None => serial_println!("[ga10bprobe5b] handshake {} @{:#x} = {:#010x} sample={} changed_since_last={}", name, addr, v, s, (s > 1 && v != last[k]) as u32),
            }
            last[k] = v;
        }
        if s < SAMPLES { settle_ms(SETTLE_MS); }
    }
    // The PMU verdict from its own cpuctl, the two apertures agreeing or not.
    let v1 = last[3];
    let f2 = last[7];
    let pmu_arm = if unreadable_reason(v1).is_some() && unreadable_reason(f2).is_some() {
        "PMU-UNREADABLE"
    } else if (unreadable_reason(v1).is_none() && (v1 >> PRISCV_CPUCTL_HALTED_BIT) & 1 == 0) || (unreadable_reason(f2).is_none() && (f2 >> PRISCV_CPUCTL_HALTED_BIT) & 1 == 0) {
        "PMU-RUNNING"
    } else {
        "PMU-HALTED"
    };
    serial_println!("[ga10bprobe5b] handshake summary: pmu_cpuctl_v1={:#010x} pmu_falcon2_cpuctl={:#010x} gsp_mailbox0={:#010x} gsp_mailbox1={:#010x} pmu_mailbox0={:#010x} pmu_mailbox1={:#010x} changes=[{} {} {} {} {} {} {} {}] unreadable=[{} {} {} {} {} {} {} {}] samples={} -> {}", v1, f2, last[0], last[1], last[5], last[6], changed[0], changed[1], changed[2], changed[3], changed[4], changed[5], changed[6], changed[7], unread[0], unread[1], unread[2], unread[3], unread[4], unread[5], unread[6], unread[7], SAMPLES, pmu_arm);
}
