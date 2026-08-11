//! GEN7-3D rungs R0/R1 — Ivy Bridge GT2 render-engine reconnaissance, READ-ONLY.
//!
//! Design of record: `~/unaos-bench/scratch/gr25/GEN7-3D-draft.md` (GR25). This module
//! implements that ladder's **R1 leg only**: a read-only census that answers "is the GT
//! window alive, is there a forcewake block where the draft guessed, what is in the GGTT,
//! and where is stolen memory". It performs **zero MMIO writes and zero config writes**.
//!
//! ## The structural promise
//!
//! **This module cannot black Peter's panel.** Two independent reasons, and both hold:
//!
//! 1. It writes nothing. Every access below is a `read_volatile` or a PCI config *read*.
//!    ⚠ Stated precisely, because the loose version of this sentence was wrong: **reads
//!    only, and the one display-block offset this file touches is read, never written.**
//!    That offset is `PCH_PP_CONTROL` (`0xC7204`) — a South Display Engine Panel Power
//!    Sequencer register, and unambiguously a display register. It is in the control frame
//!    precisely *because* it is display-block: the PPS sits outside the GT power well, so
//!    reading it separates "BAR0 is dead" from "the GT is dead". "No display register at
//!    any offset" would be a false claim; "no display register is written" is the true one,
//!    and it is the one that carries the safety argument.
//! 2. Even a write could not reach the panel: the panel is the Kepler's on this machine
//!    (`igpu.rs` ~778; Boot AS shows every iGPU pipe and plane reading `0x00000000`).
//!
//! ## Wall D — the instrument that cannot fail (draft §4.4)
//!
//! A powered-down GT may return `0x00000000` for every read. Therefore **a zero-compare is
//! never a verdict here**. Three defences, all present below:
//!
//! * **A control frame with generator-checked exact values** (`CONTROL_FRAME`): registers
//!   whose *exact* content is known from metal or from the part's own ID. They live
//!   OUTSIDE the GT (PCI config space; the PCH south display block), so they separate
//!   "BAR0 is dead" from "the GT is dead" — the AMBIGUOUS arm the draft's R1 demands.
//!   `PCH_PP_CONTROL` carries `0xABCD` in its upper half: sixteen bits of entropy that no
//!   dead bus, no floating line and no zero-filled window can produce by accident.
//! * **A read-twice stability leg**: every GT register is read twice. A register whose two
//!   reads DIFFER is positive proof of life that no zero-compare can fake, and it is
//!   counted separately (`varies=`).
//! * **A three-way classification, never a boolean**: every read lands in exactly one of
//!   `structured` / `zero` / `allones`. The GGTT census uses its own four-way scheme —
//!   `valid` / `empty` / `allones` / `malformed` — resting on a *content invariant* (PTE
//!   present-bit set AND a non-zero frame number), not on `!= 0`.
//!
//! Where the evidence does not reach, the verdict says `ambiguous` or `void`. It does not
//! guess. R1 is read-only by construction and therefore **cannot settle G1** — only the
//! draft's R2 (the forcewake acquire, the first write in the ladder) can, and this rung
//! exists to tell R2 whether it is worth flying and at which offsets.
//!
//! ## Citation classes (draft §0)
//!
//! `[PINNED]` verified this session against a public Intel PRM, URL + section in the
//! comment. `[BDW-ONLY]` / `[CHV-ONLY]` pinned on LATER silicon and **carried here as a
//! hypothesis to be read, never written**. `[EXT-UNPINNED]` recollection, unverified.
//! `[METAL]` observed on this MacBookPro10,1 in a named capture.

// x86_64 only. `arroyo`'s `arm_features` additionally strips `gen7` from aarch64 builds so
// the feature does not shift the aarch64 `-Cmetadata` fingerprint; this file-level gate is
// the belt to that braces, and it is what makes "not one byte of aarch64 code" a property
// of the source rather than of the build script.
//
// `serial_println!` is `#[macro_export]`ed at the crate root and is therefore in scope
// without an import, exactly as in `igpu.rs` next door — importing it makes the name
// ambiguous with the macro-namespace prelude entry.
#![cfg(target_arch = "x86_64")]

/// Register offsets into BAR0 (GTTMMADR) of BDF `0:2.0`, each tagged with its pin status.
///
/// The IVB PRM gives the register space for the ring blocks as `MMIO: 0/2/0` — that is
/// BAR0 of device `0:2.0`, exactly the window `igpu::init` maps and publishes.
pub mod g7regs {
    // ---- [PINNED] ------------------------------------------------------------------
    // Intel OpenSource HD Graphics Programmer's Reference Manual, Volume 1 Part 3:
    // "Graphics Core — Memory Interface and Commands for the Render Engine", For the 2012
    // Intel Core Processor Family (Ivy Bridge), May 2012 Rev 1.0,
    // Doc Ref # IHD-OS-V1 Pt 3 - 05 12.
    // URL: https://www.x.org/docs/intel/IVB/IHD_OS_Vol1_Part3.pdf
    // §1.1.11 "RINGBUF — Ring Buffer Registers", pp. 74-79. Register Space "MMIO: 0/2/0".
    //   §1.1.11.1 RING_BUFFER_TAIL    p.75 — RCS 02030h  VCS 12030h  BCS 22030h
    //   §1.1.11.2 RING_BUFFER_HEAD    p.76 — RCS 02034h  VCS 12034h  BCS 22034h
    //   §1.1.11.3 RING_BUFFER_START   p.77 — RCS 02038h  VCS 12038h  BCS 22038h
    //   §1.1.11.4 RING_BUFFER_CONTROL p.78 — RCS 0203Ch  VCS 1203Ch  BCS 2203Ch
    //   §1.1.11.5 UHPTR               p.80 — RCS 02134h              BCS 22134h
    //
    // This PINS draft claim G3 exactly as written, and it also RETIRES the uncited status
    // of the tree's own `igpu::regs::BLT_RING_*` block (`igpu.rs` ~89-93): those four
    // offsets are the BCS column above.
    pub const RCS_RING_TAIL: usize = 0x02030;
    pub const RCS_RING_HEAD: usize = 0x02034;
    pub const RCS_RING_START: usize = 0x02038;
    pub const RCS_RING_CTL: usize = 0x0203C;
    pub const RCS_UHPTR: usize = 0x02134;

    pub const VCS_RING_TAIL: usize = 0x12030;
    pub const VCS_RING_HEAD: usize = 0x12034;
    pub const VCS_RING_START: usize = 0x12038;
    pub const VCS_RING_CTL: usize = 0x1203C;

    pub const BCS_RING_TAIL: usize = 0x22030;
    pub const BCS_RING_HEAD: usize = 0x22034;
    pub const BCS_RING_START: usize = 0x22038;
    pub const BCS_RING_CTL: usize = 0x2203C;
    pub const BCS_UHPTR: usize = 0x22134;

    // [PINNED, with a caveat that matters] Same document, **§1.1.10.9 "INSTPM" —
    // Sync-Flush Workaround (VT-d)**, pp. 70-71.
    //
    // ⚠ CITATION CORRECTED (review of 055c0e58). The first cut of this comment attributed
    // the sequence below to "§1.1.11.4 programming notes ... and p. 79" and called it "the
    // IVB PRM's ONLY documented wake sequence". Both halves were wrong, and the second was
    // the dangerous one. What p.79 §1.1.11.4 actually gives is the Force-Wakeup *prose*
    // ("SW should set the Force Wakeup bit to prevent GT from entering C6") with **no
    // register named** — that is the G1 requirement, and it is cited on RCS_RING_CTL above.
    // The four MMIO steps below are a different thing entirely: they are the VT-d
    // Sync-Flush workaround, and they **wake the command streamer as a SIDE EFFECT** of
    // draining it. Intel does not present them as a general-purpose forcewake protocol,
    // and this module does not either.
    //   Write 0x2050 = 0x00010001  (disable sequence)
    //   Write 0x2700 = 0x00000000  ("Wake up CS but don't do anything")
    //   Poll  0x22AC[3:0] == 0     ("Guarantees render pipe is awake")
    //   Write 0x2050 = 0x00010000  (enable sequence, to re-enter RC6)
    // Note the 0x00010001 / 0x00010000 form: upper-16-as-write-enable-mask semantics ARE
    // in use on Ivy Bridge, stated generically in the same volume (~p.66): "Must be set to
    // modify corresponding data bit. Reads to this field returns zero." That much is a
    // clean, separable pin and it survives the correction.
    //
    // **R2's caveat, stated here so it cannot be lost between documents:** these four steps
    // are the closest thing to a wake that Intel published for Gen7, and they are worth
    // TRYING — but R2 must not treat them as a general wake sequence, must not assume the
    // CS stays awake after step 4 (step 4 exists to let RC6 back IN), and must report
    // `verdict=void` rather than `no-ack` if 0x22AC never clears, because a workaround
    // borrowed out of its stated context failing tells us about the borrowing, not about
    // the GT. The honest position after R0.1 is that **Gen7's forcewake register block is
    // undocumented** — 0xA188 is Broadwell's (see below) — and R2 is an experiment against
    // that gap, not an implementation of a spec.
    //
    // R1 READS all three registers and writes none.
    pub const INSTPM: usize = 0x02050;
    pub const RCS_WAKE: usize = 0x02700;
    pub const RENDER_IDLE_POLL: usize = 0x022AC;

    // ---- [BDW-ONLY] / [CHV-ONLY] — HYPOTHESES, READ ONLY, NEVER WRITTEN ------------
    //
    // ⚠ The draft's G2 says the forcewake handshake sits "in the 0x0A180-class block".
    // That is **NOT confirmable against Ivy Bridge documentation**. The complete
    // 16-volume IVB PRM set was searched this session (Vol1 Pt1-7, Vol2 Pt1-2,
    // Vol3 Pt1-4, Vol4 Pt1-3) for FORCEWAKE / FORCE_WAKE / GTFIFO / A188h / 130044 /
    // 130090 / "power well" / "power gat": the ONLY hits are the "Force Wakeup bit" prose
    // cited above. **Intel never published the Gen7 GT power/forcewake register block.**
    // Vol 1 Part 6 is titled "GT Interface Register" but stops at MBCunit/GDTunit/GCPunit
    // config (IDICR, SNPCR, UCGCTLn, GDRST) and never reaches the GPM block.
    //
    // So these offsets are pinned only on LATER silicon and are carried here for one
    // purpose: to find out, read-only, whether anything decodes at them on THIS part.
    // A read is free and reversible; a write to a register we cannot pin on this
    // generation is not, and R1 does not make one. Sharper still: on CHV/BSW, 0x0A188 is
    // SCRATCH1 — an unrelated ECO scratch register — so the offset is not even stable
    // across the Gen8 family, and treating it as Gen7 forcewake would be driver-lore.

    // [BDW-ONLY] Intel Open Source Graphics PRM, Volume 2c: Command Reference: Registers
    // (Broadwell), Doc Ref # IHD-OS-BDW-Vol 2c-11.15.
    // URL: https://cdrdv2-public.intel.com/690789/intel-gfx-prm-osrc-bdw-vol-02c-commandreference-registers.pdf
    // p.493 "Force Wake Request for Multiple Threads with Mask" (FORCE_WAKE),
    //       Register Space MMIO 0/2/0, Address 0A188h. Upper 16 bits are the write mask.
    pub const HYP_FORCEWAKE_MT: usize = 0x0A188;
    // [BDW-ONLY] same doc p.703, GTSP1 "GT Scratch Pad 1", Address 130044h:
    //       "[15:0] Multiple Force Wake: GT programs this field with the multiple force
    //        wake status." This is FORCE_WAKE's ack partner on BDW — note it is NOT an
    //       0xA18x address, which is the specific part of G2 that is wrong as drafted.
    pub const HYP_FORCEWAKE_MT_ACK: usize = 0x130044;
    // [BDW-ONLY] same doc p.656, GTFORCEAWAKE "GT Force Awake", Address 130090h:
    //       "This field is no longer used. The multiple force wake mechanism has replaced
    //        it. Refer to MULTIFORCEWAKE 0xA188." Read as the legacy candidate.
    pub const HYP_GTFORCEAWAKE: usize = 0x130090;
    // [BDW-ONLY] same doc p.605, MISC_CTRL0 (GPM Control), Address 0A180h. Read to show
    // what, if anything, lives at the base of the block the draft named — and to record
    // that 0xA180 is GPM control, NOT forcewake, even on the silicon where it is pinned.
    pub const HYP_MISC_CTRL0: usize = 0x0A180;

    // [CHV-ONLY] Intel Open Source Graphics PRM, Volume 2c: Command Reference: Registers
    // (Cherryview/Braswell), Doc Ref # IHD-OS-CHV-BSW-Vol 2c-10.15.
    // URL: https://cdrdv2-public.intel.com/689936/intel-gfx-bspec-osrc-chv-bsw-vol-2-c-command-reference-registers.pdf
    // pp.451-453 GTFIFOCTL, Address 120008h. This is the closest Intel ever came to
    // documenting the draft's G1 failure mode, and it is worth quoting because it is the
    // mechanism G1 asserts:
    //   bit 13 GT_FIFO_PRI_POLICY: "If WakeFIFO threshold is hit: a. Drop IOSF Primary
    //          writes to WakeFIFO b. IOSF Primary reads targeting WakeFIFO return 1s."
    //   and: "Starting with Gen8 Gfx, the driver is required to ensure the targeted power
    //          well is alive before initiating an access outside shadow register space."
    // ⚠ Two corrections to the draft fall out of this. (a) The documented dead-well read
    // value is ONES, not zeros — so `0xFFFFFFFF` is as much a G1 signature as
    // `0x00000000`, and this module classifies both. (b) GTFIFOCTL's documented bit
    // layout is all policy bits and spares: **there is no GT_FIFO_FREE_ENTRIES count
    // field in it**, on any generation, in any Intel PRM. That half of G2 is refuted.
    pub const HYP_GTFIFOCTL: usize = 0x120008;
    // [CHV-ONLY] same doc pp.487-489, GTLC_PW_STAT, Address 130094h, bit 1 ALLOWWAKEERR:
    //       "When access to media or render is observed when ALLOWWAKE=0, the ALLOWWAKERR
    //        bit will be set." This is the nearest documented analogue of the draft's
    //       "GTFIFODBG" — which exists under that name in NO Intel PRM (IVB, HSW, BDW or
    //       CHV/BSW). G2's GTFIFODBG is refuted as a register name.
    pub const HYP_GTLC_PW_STAT: usize = 0x130094;
    // [CHV-ONLY] same doc pp.1078/1077: RENFW_REQ 1300B0h / RENFW_ACK 1300B4h — the
    // per-well render forcewake pair. "Driver must poll on the corresponding bit to
    // confirm that the well has woken." A third candidate shape, read-only.
    pub const HYP_RENFW_REQ: usize = 0x1300B0;
    pub const HYP_RENFW_ACK: usize = 0x1300B4;
}

/// Which address space a control-frame row lives in.
/// This partition is not cosmetic — the rung's whole discriminating power rests on it, and
/// the BLOCKER found in review of 055c0e58 was a verdict that ignored it. `CfgIgd` reads
/// reach the device over the PCI config mechanism and **do not depend on BAR0 decoding at
/// all**; `Mmio` reads go through the BAR0 window. A device that answers config but not
/// MMIO is precisely `bar0-dead`, and only a verdict that partitions by space can name it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Space {
    /// PCI configuration space of the IGD, at the BDF the caller passed — always alive,
    /// independent of BAR0. The BDF is *not* hardcoded here (review nit): `gpu::detect`
    /// found this device, and `0:2.0` is where it lives on this machine, not a law.
    CfgIgd(u8),
    /// A dword offset into BAR0 (GTTMMADR).
    Mmio(usize),
}

/// A generator-checked expectation: `(read & mask) == want`.
///
/// This table IS the generator the draft's Wall D demands for a read-only rung. Every row
/// carries an exact value from a cited source, so a match is a positive statement about
/// content — not the absence of a zero.
struct Expect {
    name: &'static str,
    space: Space,
    want: u32,
    mask: u32,
    src: &'static str,
    /// Does a MISMATCH on this row void the frame?
    ///
    /// Review of 055c0e58 caught the defect this field fixes: the exact-value
    /// `PCH_PP_CONTROL` row was counted toward `ctrl_fail`, so **one legitimate panel-power
    /// bit change (`0xABCD0008` → `0xABCD0009`) would have voided the control frame and
    /// burned the boot** — while the comment on that very row claimed the opposite. A
    /// control frame that a normal machine can fail is not a control frame.
    ///
    /// So the strong claim and the robust claim are separated: the **key field**
    /// (`0xABCD____`, sixteen bits of entropy that no dead bus can produce) is `critical`,
    /// and the exact low byte is `informational` — printed, MATCH/MISMATCH'd on the wire,
    /// and deliberately powerless over the verdict.
    critical: bool,
}

/// The control frame. Nothing here is a GT register, and that is the whole point: these
/// reads must succeed even if the GT is fully power-gated, so a failure convicts BAR0 (or
/// the device) rather than the power well.
const CONTROL_FRAME: &[Expect] = &[
    // [METAL, Boot AS] `[PCI-CENSUS] bdf 0:2.0` — 8086:0166, Ivy Bridge GT2. Read on the
    // config path, which does not depend on BAR0 decoding at all.
    Expect {
        name: "CFG_VID_DID_IGD",
        space: Space::CfgIgd(0x00),
        want: 0x0166_8086,
        mask: 0xFFFF_FFFF,
        src: "METAL/BootAS",
        critical: true,
    },
    // [METAL, Boot AS] `:: igpu: PP_CONTROL_PCH: 0xABCD0008 ::`. The PCH Panel Power
    // Sequencer is in the SOUTH display engine, reached through the same BAR0 window but
    // NOT behind the GT power well. The `0xABCD` unlock key in the upper half is sixteen
    // bits of entropy: a window that returns it is decoding real data. This single row is
    // the load-bearing discriminator of the whole rung.
    // ⚠ INFORMATIONAL, not critical. The exact Boot AS value including the low byte. The
    // panel power state legitimately moves, so this row is expected to mismatch on some
    // healthy boots and it must never be able to void the frame — which is exactly the bug
    // review found here: as first written this row was counted, and `0xABCD0009` would have
    // burned a boot for a correct machine.
    Expect {
        name: "PCH_PP_CONTROL_EXACT",
        space: Space::Mmio(0xC7204),
        want: 0xABCD_0008,
        mask: 0xFFFF_FFFF,
        src: "METAL/BootAS",
        critical: false,
    },
    // CRITICAL. The same register, key field only: `0xABCD` is the PPS unlock key and it
    // does not move with panel power state. Sixteen bits of entropy — this is the row that
    // actually proves the BAR0 window is decoding real data, and it is the only MMIO row
    // with a vote.
    Expect {
        name: "PCH_PP_CONTROL_KEY",
        space: Space::Mmio(0xC7204),
        want: 0xABCD_0000,
        mask: 0xFFFF_0000,
        src: "METAL/BootAS",
        critical: true,
    },
];

/// Classification of a single 32-bit read. Named exhaustively BEFORE the probe runs
/// (classified-verdict law) so no outcome falls through to a default.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cls {
    /// Non-zero and not all-ones: real content.
    Structured,
    /// `0x00000000`. AMBIGUOUS by construction — a legitimately-zero reset value and a
    /// power-gated GT are indistinguishable in one read. Never a verdict on its own.
    Zero,
    /// `0xFFFFFFFF`. Either an unclaimed/aborted bus cycle, or — per the CHV GTFIFOCTL
    /// citation — a dead-well read. Also ambiguous, and also never a verdict alone.
    AllOnes,
}

impl Cls {
    fn of(v: u32) -> Cls {
        match v {
            0x0000_0000 => Cls::Zero,
            0xFFFF_FFFF => Cls::AllOnes,
            _ => Cls::Structured,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Cls::Structured => "structured",
            Cls::Zero => "zero",
            Cls::AllOnes => "allones",
        }
    }
}

/// Running tally for a battery of GT reads.
#[derive(Default, Clone, Copy)]
struct Tally {
    n: u32,
    structured: u32,
    zero: u32,
    allones: u32,
    /// Registers whose two consecutive reads DIFFERED. Positive proof of life; a
    /// zero-compare cannot fake it.
    varies: u32,
}

impl Tally {
    fn add(&mut self, c: Cls, varied: bool) {
        self.n += 1;
        match c {
            Cls::Structured => self.structured += 1,
            Cls::Zero => self.zero += 1,
            Cls::AllOnes => self.allones += 1,
        }
        if varied {
            self.varies += 1;
        }
    }
}

#[inline(always)]
unsafe fn rd(bar0: usize, off: usize) -> u32 {
    core::ptr::read_volatile((bar0 + off) as *const u32)
}

/// One battery row: read twice, classify, print, tally. **Two reads, no writes.**
unsafe fn probe(bar0: usize, blk: &str, name: &str, off: usize, pin: &str, t: &mut Tally) -> u32 {
    let a = rd(bar0, off);
    let b = rd(bar0, off);
    let varied = a != b;
    let c = Cls::of(a);
    t.add(c, varied);
    serial_println!(
        ":: gen7: gt blk={} name={} off={:06X} v0={:08X} v1={:08X} cls={} varies={} pin={} ::",
        blk,
        name,
        off,
        a,
        b,
        c.name(),
        if varied { 1 } else { 0 },
        pin
    );
    a
}

/// GEN7 rung R1 — read-only reconnaissance. Called from `igpu::init` BEFORE
/// `bring_up_blt_ring`, so the GGTT census below sees firmware's state and not ours.
///
/// `bar0` is the already-mapped, already-`translate`-checked BAR0 virtual base that
/// `igpu::init` publishes (`igpu.rs` ~440-444); `bar0_size` is that mapping's real length
/// as the PCI enumerator reported it, and `bus`/`slot`/`func` are the BDF `gpu::detect`
/// found the IGD at.
///
/// # Safety
/// Caller guarantees `bar0` is a live MMIO mapping of at least `bar0_size` bytes of the
/// IGD's BAR0. Every access in this function is a read; the function never writes MMIO or
/// config space.
pub unsafe fn recon(bar0: usize, bar0_size: usize, bus: u8, slot: u8, func: u8) {
    serial_println!(
        ":: gen7: recon begin rung=R1 mode=read-only writes=0 bdf={}:{}.{} bar0_size={} ladder=GEN7-3D ::",
        bus,
        slot,
        func,
        bar0_size
    );

    // ---- Parachute -------------------------------------------------------------------
    // An accelerator probe must degrade, never kill the boot (`igpu.rs` ~556-559). Every
    // refusal names itself and returns; nothing downstream depends on this function.
    //
    // Review nit fixed here: the early return used to skip the `next=`/`note=` line, so a
    // refused boot's log lacked the one sentence that says what to do about it. Every exit
    // from this function now prints it.
    if bar0 == 0 {
        serial_println!(":: gen7: recon verdict=refused-no-bar0 — BAR0 unpublished; nothing claimed ::");
        serial_println!(
            ":: gen7: recon next=STOP-bar0-unpublished-fix-igpu-init-mapping-first note=R1-is-read-only-and-cannot-settle-G1 ::"
        );
        serial_println!(":: gen7: recon end ::");
        return;
    }

    // ---- R1a — the control frame (generator-checked exact values) ---------------------
    // Tallied SEPARATELY by address space. The BLOCKER review found in 055c0e58 was a
    // verdict computed from a single undifferentiated `ctrl_fail`, which made `bar0-dead`
    // unreachable: the config row and the `device-absent` test were the same read, so
    // `device-absent` always won first and the genuine bar0-dead machine (config fine, both
    // MMIO rows dead) printed `ambiguous`. Partitioning here is what makes the arm real.
    let mut cfg_pass = 0u32;
    let mut cfg_fail = 0u32;
    let mut mmio_pass = 0u32;
    let mut mmio_fail = 0u32;
    let mut info_pass = 0u32;
    let mut info_fail = 0u32;
    for e in CONTROL_FRAME {
        let v = match e.space {
            Space::CfgIgd(o) => crate::arch::pci::read_config_32(bus, slot, func, o),
            Space::Mmio(off) => rd(bar0, off),
        };
        let ok = (v & e.mask) == e.want;
        match (e.critical, e.space, ok) {
            // Informational rows are counted, printed, and given no vote.
            (false, _, true) => info_pass += 1,
            (false, _, false) => info_fail += 1,
            (true, Space::CfgIgd(_), true) => cfg_pass += 1,
            (true, Space::CfgIgd(_), false) => cfg_fail += 1,
            (true, Space::Mmio(_), true) => mmio_pass += 1,
            (true, Space::Mmio(_), false) => mmio_fail += 1,
        }
        serial_println!(
            ":: gen7: ctrl name={} space={} crit={} val={:08X} mask={:08X} want={:08X} {} src={} ::",
            e.name,
            match e.space {
                Space::CfgIgd(_) => "cfg",
                Space::Mmio(_) => "mmio",
            },
            if e.critical { 1 } else { 0 },
            v,
            e.mask,
            e.want,
            if ok { "MATCH" } else { "MISMATCH" },
            e.src
        );
    }
    let ctrl_pass = cfg_pass + mmio_pass;
    let ctrl_n = cfg_pass + cfg_fail + mmio_pass + mmio_fail;
    serial_println!(
        ":: gen7: ctrl verdict crit_pass={}/{} cfg={}/{} mmio={}/{} info_pass={} info_fail={} ::",
        ctrl_pass,
        ctrl_n,
        cfg_pass,
        cfg_pass + cfg_fail,
        mmio_pass,
        mmio_pass + mmio_fail,
        info_pass,
        info_fail
    );

    // ---- R1b — the GT battery, at PINNED Ivy Bridge offsets ---------------------------
    // Everything here is IVB-PRM-pinned (Vol 1 Part 3 §1.1.11 and pp.70-71), so a zero
    // column cannot be blamed on a wrong offset. That is why this battery, and not the
    // hypothesis battery below, is what the G1 reading is drawn from.
    let mut gt = Tally::default();
    let rcs_ctl = probe(bar0, "rcs", "RING_CTL", g7regs::RCS_RING_CTL, "IVB-PINNED", &mut gt);
    let rcs_head = probe(bar0, "rcs", "RING_HEAD", g7regs::RCS_RING_HEAD, "IVB-PINNED", &mut gt);
    probe(bar0, "rcs", "RING_TAIL", g7regs::RCS_RING_TAIL, "IVB-PINNED", &mut gt);
    probe(bar0, "rcs", "RING_START", g7regs::RCS_RING_START, "IVB-PINNED", &mut gt);
    probe(bar0, "rcs", "UHPTR", g7regs::RCS_UHPTR, "IVB-PINNED", &mut gt);

    let bcs_ctl = probe(bar0, "bcs", "RING_CTL", g7regs::BCS_RING_CTL, "IVB-PINNED", &mut gt);
    probe(bar0, "bcs", "RING_HEAD", g7regs::BCS_RING_HEAD, "IVB-PINNED", &mut gt);
    probe(bar0, "bcs", "RING_TAIL", g7regs::BCS_RING_TAIL, "IVB-PINNED", &mut gt);
    probe(bar0, "bcs", "RING_START", g7regs::BCS_RING_START, "IVB-PINNED", &mut gt);
    probe(bar0, "bcs", "UHPTR", g7regs::BCS_UHPTR, "IVB-PINNED", &mut gt);

    probe(bar0, "vcs", "RING_CTL", g7regs::VCS_RING_CTL, "IVB-PINNED", &mut gt);
    probe(bar0, "vcs", "RING_HEAD", g7regs::VCS_RING_HEAD, "IVB-PINNED", &mut gt);
    probe(bar0, "vcs", "RING_TAIL", g7regs::VCS_RING_TAIL, "IVB-PINNED", &mut gt);
    probe(bar0, "vcs", "RING_START", g7regs::VCS_RING_START, "IVB-PINNED", &mut gt);

    // The IVB-documented wake path's own three registers. R2 may WRITE 0x2050/0x2700;
    // R1 only looks. 0x22AC is the poll target ("Guarantees render pipe is awake") and is
    // the single most interesting read in the battery: bits 3:0 are a live pipe-state
    // field, so a `varies=1` here would be proof of a running GT.
    let instpm = probe(bar0, "gt", "INSTPM", g7regs::INSTPM, "IVB-PINNED", &mut gt);
    probe(bar0, "gt", "RCS_WAKE", g7regs::RCS_WAKE, "IVB-PINNED", &mut gt);
    let idle = probe(bar0, "gt", "RENDER_IDLE_POLL", g7regs::RENDER_IDLE_POLL, "IVB-PINNED", &mut gt);

    serial_println!(
        ":: gen7: gt verdict n={} structured={} zero={} allones={} varies={} ::",
        gt.n,
        gt.structured,
        gt.zero,
        gt.allones,
        gt.varies
    );

    // A named cross-check on the ring blocks specifically, because these are the four
    // registers the tree's own never-run BLT bring-up programs. If the two engines'
    // CONTROL registers read IDENTICAL and non-zero, that is a decode-folding smell (one
    // window aliased over both), and it is reported as a flag — never as a verdict.
    let ring_alias = rcs_ctl == bcs_ctl && rcs_ctl != 0 && rcs_ctl != 0xFFFF_FFFF;
    serial_println!(
        ":: gen7: gt ringcheck rcs_ctl={:08X} bcs_ctl={:08X} rcs_head={:08X} instpm={:08X} idle_lo4={:X} alias_smell={} ::",
        rcs_ctl,
        bcs_ctl,
        rcs_head,
        instpm,
        idle & 0xF,
        if ring_alias { 1 } else { 0 }
    );

    // ---- R1c — forcewake / GTFIFO PRESENCE probe, hypothesis offsets, READ ONLY -------
    // ⚠ Not one of these offsets is pinned on Ivy Bridge (see g7regs). They are read to
    // find out whether anything decodes there. No reading about G1 is drawn from this
    // battery — a structured value here is a lead for R2, not a fact.
    let mut fw = Tally::default();
    probe(bar0, "fw", "HYP_FORCEWAKE_MT", g7regs::HYP_FORCEWAKE_MT, "BDW-ONLY", &mut fw);
    probe(bar0, "fw", "HYP_FORCEWAKE_MT_ACK", g7regs::HYP_FORCEWAKE_MT_ACK, "BDW-ONLY", &mut fw);
    probe(bar0, "fw", "HYP_GTFORCEAWAKE", g7regs::HYP_GTFORCEAWAKE, "BDW-ONLY", &mut fw);
    probe(bar0, "fw", "HYP_MISC_CTRL0", g7regs::HYP_MISC_CTRL0, "BDW-ONLY", &mut fw);
    probe(bar0, "fw", "HYP_GTFIFOCTL", g7regs::HYP_GTFIFOCTL, "CHV-ONLY", &mut fw);
    probe(bar0, "fw", "HYP_GTLC_PW_STAT", g7regs::HYP_GTLC_PW_STAT, "CHV-ONLY", &mut fw);
    probe(bar0, "fw", "HYP_RENFW_REQ", g7regs::HYP_RENFW_REQ, "CHV-ONLY", &mut fw);
    probe(bar0, "fw", "HYP_RENFW_ACK", g7regs::HYP_RENFW_ACK, "CHV-ONLY", &mut fw);
    serial_println!(
        ":: gen7: fw-probe verdict n={} structured={} zero={} allones={} varies={} pinned_for_ivb=0 wrote=0 ::",
        fw.n,
        fw.structured,
        fw.zero,
        fw.allones,
        fw.varies
    );

    // ---- R1d — the GGTT census (never read on this machine) --------------------------
    // `igpu::regs::GTT_BASE = 0x200000` is 2 MiB into a 4 MiB BAR0 [METAL, Boot AS:
    // "[GPU] BAR0: 0xC1400000 (Size: 4194304 bytes)"], so the PTE array is plausibly the
    // upper 2 MiB = 524288 dword PTEs = 2 GiB of GGTT space. That interpretation is
    // [EXT-UNPINNED] and this rung is what confirms or kills it: `last_slot` below is the
    // final dword of BAR0, and what it returns says whether the array really runs that
    // far. Sampled, not walked — 524288 serial lines is not a census, it is a denial of
    // service on the bench.
    //
    // ⚠ `GTT_BASE = 0x200000` is [EXT-UNPINNED] (review). It is copied from
    // `igpu::regs::GTT_BASE`, where it ships **uncited**; R0.1 pinned the ring blocks and
    // G8, but it did NOT find a PRM statement placing the GGTT PTE array at BAR0+2 MiB on
    // Ivy Bridge. So `ggtt_valid=` below is *conditional on that offset being right*, and
    // the witness says so on the wire (`base_pin=unpinned`) rather than in a comment only
    // — an `ggtt_valid=0` reading is equally consistent with "the array is empty" and
    // "we are not reading the array". R1 cannot separate those; naming it is the job.
    const GTT_BASE: usize = 0x200000; // igpu::regs::GTT_BASE — [EXT-UNPINNED]
    const SLOTS: usize = 524_288; // (4 MiB - 2 MiB) / 4, per the derivation above

    // The guard below used to compare against a hardcoded 4 MiB literal — i.e. against the
    // size we EXPECT rather than the size we were GIVEN (review). On a machine that
    // enumerates a smaller BAR0 that guard would have waved through reads past the end of
    // the mapping. It now uses `bar0_size`, and a short BAR0 refuses the leg by name
    // instead of sampling into whatever follows the window.
    if bar0_size < GTT_BASE + SLOTS * 4 {
        serial_println!(
            ":: gen7: ggtt verdict=refused-bar0-too-small have={} need={} — the 2MiB-MMIO+2MiB-GGTT derivation does not hold on this part; no GGTT reading claimed ::",
            bar0_size,
            GTT_BASE + SLOTS * 4
        );
    }
    let sample: [usize; 12] = [
        0,
        1,
        2,
        3,
        16,
        256,
        4_096,
        65_536,
        131_072,
        262_144,
        SLOTS - 2,
        SLOTS - 1,
    ];
    let mut valid = 0u32;
    let mut empty = 0u32;
    let mut ones = 0u32;
    let mut malformed = 0u32;
    let mut first_valid: i32 = -1;
    let mut first_valid_pte = 0u32;
    for &s in sample.iter() {
        let off = GTT_BASE + s * 4;
        // Bounds guard against the ACTUAL mapping length, not against the expected one.
        if off + 4 > bar0_size {
            serial_println!(
                ":: gen7: ggtt slot={} off={:06X} SKIPPED=out-of-bar0 bar0_size={} ::",
                s,
                off,
                bar0_size
            );
            continue;
        }
        let pte = rd(bar0, off);
        // The content invariant, and it is deliberately NOT `!= 0`: a Gen7 GGTT PTE is one
        // dword carrying `(phys & ~0xFFF) | valid` (draft G6, [EXT-UNPINNED], and the
        // shape the tree itself writes at `igpu.rs` ~835). So a PRESENT entry must have
        // bit 0 set AND a non-zero frame number. An entry that is non-zero with bit 0
        // clear is neither present nor empty — it is `malformed`, and it is exactly the
        // case a `!= 0` test would silently miscount as "populated".
        let cls = if pte == 0xFFFF_FFFF {
            ones += 1;
            "allones"
        } else if pte == 0 {
            empty += 1;
            "empty"
        } else if (pte & 1) != 0 && (pte & 0xFFFF_F000) != 0 {
            valid += 1;
            if first_valid < 0 {
                first_valid = s as i32;
                first_valid_pte = pte;
            }
            "valid"
        } else {
            malformed += 1;
            "malformed"
        };
        serial_println!(
            ":: gen7: ggtt slot={} off={:06X} pte={:08X} pfn={:05X} cls={} ::",
            s,
            off,
            pte,
            pte >> 12,
            cls
        );
    }
    serial_println!(
        ":: gen7: ggtt verdict sampled={} valid={} empty={} allones={} malformed={} first_valid={} first_pte={:08X} base={:06X} base_pin=unpinned ::",
        sample.len(),
        valid,
        empty,
        ones,
        malformed,
        first_valid,
        first_valid_pte,
        GTT_BASE
    );

    // ---- R1e — stolen memory (GGC / BDSM), from the host bridge -----------------------
    // Read in `gmux_igd_switch` (`igpu.rs` ~1146-1147) — a path that has NEVER run, so
    // these two values have never been printed on this machine. They are host-bridge
    // CONFIG reads at BDF 0:0.0, so they are alive regardless of the GT's power state and
    // regardless of BAR0. R3 needs them to know what range it may claim.
    //
    // ⚠ The FIELD DECODE of GGC (GMS/GGMS) is [EXT-UNPINNED] — it was NOT found in the
    // IVB PRM this session, so this rung prints the RAW dwords and applies only invariants
    // it can defend, and it says `dec=unpinned` on the wire so no reader mistakes an
    // arithmetic guess for a decode. Naming what we cannot decode is the point.
    let ggc = crate::arch::pci::read_config_32(0, 0, 0, 0x50);
    let bdsm = crate::arch::pci::read_config_32(0, 0, 0, 0xB0);
    let mch_id = crate::arch::pci::read_config_32(0, 0, 0, 0x00);
    // BDSM's base is in the upper bits; the low bits carry a lock/reserved field.
    //
    // ⚠ The first cut printed `aligned_1mb` computed as `(bdsm & 0xFFF00000) & 0xFFFFF == 0`
    // — a TAUTOLOGY (review): masking off the low 20 bits and then testing that the low 20
    // bits are clear is always true, so the field was a `1` that could never be a `0`. An
    // instrument that cannot fail is the exact defect Wall D is about, and it got into the
    // one leg where the citation discipline was weakest.
    //
    // Replaced with a test of the RAW dword, which can genuinely go either way, and the raw
    // low field is printed alongside so the reader can see what it decided on. No claim is
    // made about which answer is healthy — the low bits are [EXT-UNPINNED] (`dec=unpinned`)
    // and this rung's job is to put the first-ever reading of GGC/BDSM on the wire, not to
    // grade it.
    let bdsm_base = bdsm & 0xFFF0_0000;
    let bdsm_low = bdsm & 0x000F_FFFF;
    let raw_low_clear = bdsm_low == 0;
    serial_println!(
        ":: gen7: dsm mch_id={:08X} ggc={:08X} bdsm={:08X} bdsm_base={:08X} bdsm_low={:05X} raw_low_clear={} dec=unpinned ::",
        mch_id,
        ggc,
        bdsm,
        bdsm_base,
        bdsm_low,
        if raw_low_clear { 1 } else { 0 }
    );

    // ---- The rung verdict ------------------------------------------------------------
    // Every arm is named here, in source, before the probe ran. `ambiguous` and
    // `void` are first-class outcomes, not fallthroughs.
    //
    //  device-absent  the config-space ID is wrong          → nothing else is claimed
    //  bar0-dead      config ID right, MMIO control wrong    → BAR0 is not where we think;
    //                                                          NO GT reading may be drawn
    //  gt-alive       control frame clean AND the pinned GT battery shows content or
    //                 motion                                 → G1 is WEAKENED; R2 becomes
    //                                                          a confirmation, not a gate
    //  gt-dark        control frame clean AND every pinned GT read is zero-or-ones
    //                                                        → the G1 SIGNATURE. R2 is the
    //                                                          decisive rung and is worth
    //                                                          a boot.
    //  ambiguous      the critical MMIO rows DISAGREE with each other → read, conclude
    //                                                          nothing
    //  gt-maybe       control frame clean, but the GT evidence is a single structured
    //                 register and no motion                 → too thin to skip R2
    //
    // ⚠ TWO DEFECTS FIXED HERE after review of 055c0e58. Both were in this block, and both
    // were of the same family: a verdict that could not reach a state it named.
    //
    // (1) BLOCKER — `bar0-dead` was UNREACHABLE. It required `ctrl_fail == ctrl_n`, i.e.
    //     every frame row failing including the config row — but the config row is the SAME
    //     READ as `cfg_id_ok`, so `device-absent` always won the `if` first. The machine the
    //     arm exists for (config space answers `8086:0166`, both MMIO rows dead) fell
    //     through to `ambiguous`, which is the one word that would have stopped the next
    //     boot from being spent on the actual fault. The fix is to partition by ADDRESS
    //     SPACE, which is the distinction the arm was always about: `bar0-dead` is "the
    //     config row passes AND every critical MMIO row fails". `ambiguous` now means only
    //     what its name says — the MMIO rows disagree among themselves.
    //
    // (2) `gt-alive` fired on ONE stray structured bit. A single register reading
    //     `0x00000001` gave `structured > 0`, and the rung would have printed `gt-alive`
    //     and `next=R3`, **skipping R2 — the highest-value rung in the ladder** — on the
    //     strength of one bit. The dead side of this test is defended three ways (control
    //     frame, three-way classification, read-twice); the alive side had a one-bit
    //     threshold. It now needs real evidence — motion, or at least two structured
    //     registers — and everything short of that lands in `gt-maybe`, which routes to R2
    //     exactly like `gt-dark` does. **When in doubt, fly the decisive rung.**
    let cfg_id_ok =
        crate::arch::pci::read_config_32(bus, slot, func, 0x00) == 0x0166_8086;
    let mmio_crit_n = mmio_pass + mmio_fail;
    let verdict = if !cfg_id_ok {
        "device-absent"
    } else if cfg_fail == 0 && mmio_crit_n > 0 && mmio_pass == 0 {
        // The device answers config space and NOTHING in the BAR0 window. This is the
        // arm the blocker made unreachable.
        "bar0-dead"
    } else if cfg_fail != 0 || mmio_fail != 0 {
        "ambiguous"
    } else if gt.varies > 0 || gt.structured >= 2 {
        "gt-alive"
    } else if gt.structured > 0 {
        "gt-maybe"
    } else {
        "gt-dark"
    };
    serial_println!(
        ":: gen7: recon verdict={} ctrl={}/{} gt_structured={}/{} gt_varies={} gt_allones={} fw_structured={}/{} ggtt_valid={} rung=R1 writes=0 ::",
        verdict,
        ctrl_pass,
        ctrl_n,
        gt.structured,
        gt.n,
        gt.varies,
        gt.allones,
        fw.structured,
        fw.n,
        valid
    );
    // What the NEXT rung should be, printed by the rung that earned the right to say it.
    // R1 is read-only and therefore cannot settle G1; saying so on the wire is the
    // honest end of a read-only rung.
    serial_println!(
        ":: gen7: recon next={} note=R1-is-read-only-and-cannot-settle-G1 ::",
        match verdict {
            // The INSTPM sequence is the VT-d Sync-Flush workaround (Vol 1 Pt 3 §1.1.10.9),
            // NOT a documented forcewake protocol — it wakes the CS as a side effect. The
            // name says `sync-flush-wa` so no one downstream reads it as a spec.
            "gt-dark" => "R2-try-IVB-sync-flush-wa-as-wake(0x2050/0x2700/poll-0x22AC)",
            "gt-maybe" => "R2-same-evidence-too-thin-to-skip-it",
            "gt-alive" => "R3-ggtt-claim(read-only)-then-R4-blt-execute",
            "bar0-dead" => "STOP-fix-bar0-mapping-first",
            "device-absent" => "STOP-no-igpu-on-this-boot",
            _ => "STOP-critical-mmio-rows-disagree-investigate-before-any-write",
        }
    );
    serial_println!(":: gen7: recon end ::");
}
