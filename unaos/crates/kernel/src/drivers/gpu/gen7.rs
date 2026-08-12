//! GEN7-3D rungs R1/R2/R3 — Ivy Bridge GT2 render-engine reconnaissance and the GT
//! power-well wake.
//!
//! Design of record: `~/unaos-bench/scratch/gr25/GEN7-3D-draft.md` (GR25). This module
//! implements three legs of that ladder:
//!
//! * **R1 (`recon`)** — a read-only census that answers "is the GT window alive, is there a
//!   forcewake block where the draft guessed, what is in the GGTT, and where is stolen
//!   memory". It performs **zero MMIO writes and zero config writes**.
//! * **R2 (`wake`)** — the ladder's FIRST write. It drives the IVB-legal Sync-Flush
//!   workaround path (draft §3.1 P2, IVB-V1P3 §1.1.10.9): write `INSTPM 0x2050 = 0x00010001`,
//!   write `RCS_WAKE 0x2700 = 0`, poll `0x22AC[3:0] == 0` (bounded), then **re-park**
//!   `INSTPM = 0x00010000` on every exit path. Its proof is the SAME 17-register GT battery
//!   R1 read dark reading STRUCTURED/varying afterward — a dark→live transition on the
//!   fourteen ring-block registers R2 never wrote. Three power-management writes, all
//!   reversed in-rung, and **not one display register** among them.
//! * **R3 (`forcewake`)** — the rung that goes after the power well itself. R2 flew on Boot
//!   D and came back `verdict=gt-still-dark trans_untouched=0/14 poll_ack=1`: the write went
//!   on the wire, the poll target read zero, and **nothing lit**. That is the Sync-Flush
//!   workaround doing exactly what its own PRM section says it does — draining the command
//!   streamer — and it is *not* a power-well acquire. R3 therefore drives the two
//!   **forcewake request/ack register pairs Intel actually published** (on later silicon;
//!   see the citation classes below), one at a time, each acquire released in-rung with the
//!   release **verified against the entry value**, and re-reads the same 17-register battery
//!   under each hold. It writes **two registers at most, one at a time**, both in the GT
//!   power/PM block, neither in a ring block, neither in a display block.
//! * **R4 (`claim`)** — the GGTT-claim / ring-buffer-address rung. It reads R3's verdict and
//!   branches: if R3 woke the GT it reads the RCS ring block, verifies a candidate GGTT
//!   window is entirely unowned (every pre-image zero), and then performs **one reversible
//!   PTE round-trip** — write a well-formed PTE (a real translated scratch page, not a magic
//!   number) into the first slot of the verified-empty window, read back the `0 → pte`
//!   transition, verify the neighbours did not smear, restore the whole touched
//!   neighbourhood to zero and verify it. If R3 did NOT wake the GT (`Dark` — the outcome
//!   Boot D's `gt-still-dark` makes most likely) it writes **nothing**: it runs the identical
//!   read-only census and reports `verdict=gated-on-wake` loudly, so the rung still produces a
//!   verdict and moves the GEN7-vs-Kepler question rather than silently doing nothing behind a
//!   closed well. The one write path is gated on `GtWake::reachable()`; a closed well is never
//!   written behind.
//!
//! ## The structural promise
//!
//! **This module cannot black Peter's panel.** Two independent reasons, and both hold:
//!
//! 1. R1 writes nothing; R2 writes only three GT **power-management** registers
//!    (`INSTPM 0x2050`, `RCS_WAKE 0x2700`) and re-parks `INSTPM` before it returns; R3 writes
//!    only the two **forcewake request** registers (`0x0A188`, `0x1300B0`) and releases each
//!    back to its entry value in the same rung. R4 writes **at most three GGTT PTEs** — and
//!    only on the woke branch, only into a window every slot of which it first read as zero,
//!    and all three restored to zero and re-read before it returns; on the dark branch R4
//!    writes nothing at all. No rung writes a GGTT entry that was not zero, a ring register,
//!    or anything in a display block. ⚠ Stated precisely, because the
//!    loose version of this sentence was wrong: **the one display-block offset this file
//!    touches is read, never written, in either rung.** That offset is `PCH_PP_CONTROL`
//!    (`0xC7204`) — a South Display Engine Panel Power Sequencer register, and unambiguously
//!    a display register. It is in the R1 control frame precisely *because* it is
//!    display-block: the PPS sits outside the GT power well, so reading it separates "BAR0 is
//!    dead" from "the GT is dead". "No display register at any offset" would be a false
//!    claim; "no display register is written" is the true one, and it is the one that carries
//!    the safety argument. R2's re-park is the mirror of this: the wake write is reversed on
//!    every exit path (success, timeout, short-BAR0 refusal), so the GT is returned to the
//!    exact RC6-eligible state firmware left it in.
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
//! **Wall D for R2 (`wake`).** A zero-compare cannot prove a wake either, and R2 adds a
//! sharper trap the read-only rung did not need: three of the seventeen battery registers
//! (`INSTPM`, `RCS_WAKE`, `RENDER_IDLE_POLL`) are the ones R2 itself WRITES, so a dark→live
//! transition on them proves only that the BAR0 window latched our write — not that the GT
//! power well woke. The wake verdict's liveness count is therefore drawn from the FOURTEEN
//! ring-block registers R2 never touches; a register we caused to change is not a witness.
//! `gt-woke` requires the untouched fourteen to show real motion (≥2 gone structured, or any
//! read-twice `varies`), mirroring R1's alive-side threshold so one stray latched bit cannot
//! forge a wake. `wake-void` is reserved for the poll never clearing — a Sync-Flush
//! workaround borrowed out of context failing tells us about the borrowing, not the GT
//! (draft §3.1 P2). `gt-still-dark` — ack asserts, battery unchanged — is a real, surprising
//! finding, not a failure to hide.
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

// R4's reversible GGTT claim allocates a single 4 KiB scratch page, translates it to a
// physical frame, and programs a PTE that maps it — the same allocator and translate walk
// `igpu::bring_up_blt_ring` uses for the ring page, so the PTE value is a real address, not a
// fabricated constant. The page is freed before `claim` returns — but ONLY once the GGTT
// neighbourhood is proven restored to zero; if the reversal does not verify the page is leaked,
// never handed back to the allocator while a PTE might still map it.
use alloc::alloc::{alloc_zeroed, dealloc, Layout};

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

    // ---- [BDW-ONLY] / [CHV-ONLY] — HYPOTHESES; two of them are WRITTEN by R3 -------
    //
    // ⚠ HEADING CORRECTED WHEN R3 LANDED. Through R1/R2 this block was "READ ONLY, NEVER
    // WRITTEN", and that was true. R3 breaks it deliberately for exactly two offsets —
    // `HYP_FORCEWAKE_MT` (0x0A188) and `HYP_RENFW_REQ` (0x1300B0), the two **request**
    // registers of the two forcewake pairs Intel published — and the sentence is fixed here
    // rather than left standing, because a stale safety claim is worse than none. The other
    // six offsets in this block are still read-only in every rung. The licence for writing
    // an offset pinned only on later silicon is the draft's own §0 rule: an [EXT-UNPINNED] /
    // later-silicon fact's *legal use is as a hypothesis this ladder tests on our own
    // silicon*, and §5's write-rung rules are what make the test safe — one candidate at a
    // time, released in-rung, the release verified against the entry value, and no display
    // register anywhere near it.
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
    //
    // ⚠ [METAL, Boot D 2026-08-11] — the single most important reading R1 produced, and R3
    // is built on it. `:: gen7: gt blk=fw name=HYP_GTFIFOCTL off=120008 v0=0000003F
    // v1=0000003F cls=structured ::` — **this was the ONLY structured register in the whole
    // 25-register probe.** Every ring-block register, every 0xA18x offset and every other
    // 0x13xxxx offset read `0x00000000`. So on THIS part, at THIS offset, something decodes
    // and returns a stable non-zero pattern while the 0x2xxxx ring block is dark. Two
    // consequences, and neither is a decode of the value (we have no IVB field layout for
    // it, and `0x3F` is NOT claimed to be a free-entry count — that would be the refuted
    // half of G2 sneaking back in):
    //   1. The BAR0 window reaches the 0x12xxxx block, so a zero at 0x1300xx is a statement
    //      about that register, not about the mapping.
    //   2. There is a live GT-wrapper block OUTSIDE whatever gates the ring registers.
    // R3 reads this register in **every** column for exactly that reason: it is the nearest
    // thing this machine has to an always-on GT witness, so a CHANGE in it across a
    // forcewake acquire is evidence, and its steadiness is a decode control.
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

// ===================================================================================
// R2 — the wake. The FIRST write in the GEN7 ladder.
// ===================================================================================

/// The 17-register GT battery — the SAME registers, in the SAME order, that R1's `probe`
/// battery read dark (`gt_structured=0/17` on Boot B). R2 reads it twice, once BEFORE the
/// wake and once while the wake is held, and the dark→live delta between the two columns is
/// the verdict.
///
/// The fourth field is `touched`: `true` for the three registers R2 itself writes during the
/// sequence (`INSTPM`, `RCS_WAKE`, `RENDER_IDLE_POLL` — the poll target). A dark→live
/// transition on a `touched` register proves only that the BAR0 window latched our own write,
/// so those three are **excluded from the wake verdict's liveness count** (Wall D for R2).
/// The fourteen ring-block registers we never write are the honest witness: a zero→structured
/// on one of them is engine life, not our footprint.
const GT_BATTERY: &[(&str, &str, usize, bool)] = &[
    ("rcs", "RING_CTL", g7regs::RCS_RING_CTL, false),
    ("rcs", "RING_HEAD", g7regs::RCS_RING_HEAD, false),
    ("rcs", "RING_TAIL", g7regs::RCS_RING_TAIL, false),
    ("rcs", "RING_START", g7regs::RCS_RING_START, false),
    ("rcs", "UHPTR", g7regs::RCS_UHPTR, false),
    ("bcs", "RING_CTL", g7regs::BCS_RING_CTL, false),
    ("bcs", "RING_HEAD", g7regs::BCS_RING_HEAD, false),
    ("bcs", "RING_TAIL", g7regs::BCS_RING_TAIL, false),
    ("bcs", "RING_START", g7regs::BCS_RING_START, false),
    ("bcs", "UHPTR", g7regs::BCS_UHPTR, false),
    ("vcs", "RING_CTL", g7regs::VCS_RING_CTL, false),
    ("vcs", "RING_HEAD", g7regs::VCS_RING_HEAD, false),
    ("vcs", "RING_TAIL", g7regs::VCS_RING_TAIL, false),
    ("vcs", "RING_START", g7regs::VCS_RING_START, false),
    ("gt", "INSTPM", g7regs::INSTPM, true),
    ("gt", "RCS_WAKE", g7regs::RCS_WAKE, true),
    ("gt", "RENDER_IDLE_POLL", g7regs::RENDER_IDLE_POLL, true),
];

const BATTERY_N: usize = 17;

#[inline(always)]
unsafe fn wr(bar0: usize, off: usize, v: u32) {
    core::ptr::write_volatile((bar0 + off) as *mut u32, v);
}

/// One witnessed write: pre-read the target, write, post-read it, print all three. The
/// post-read is the reversibility/latch evidence — on this part's mask-write registers a
/// read returns the DATA bits with the mask field reading zero (IVB-V1P3 ~p.66, "Reads to
/// this field returns zero"), so a healthy decode of `INSTPM=0x00010001` post-reads
/// `0x00000001`, and the re-park post-reads `0x00000000`. A power-gated window post-reads
/// `0x00000000` regardless — which is itself a reading, not a hang.
///
/// `tag` names the RUNG on the wire (`wake` for R2, `r3` for R3) so a log reader never has to
/// infer which rung a write belongs to from its position in the capture.
///
/// Returns the POST-read. R2 discards it (its evidence is the battery); R3 needs it, because
/// whether the target register reads back its own write at all is what decides whether R3's
/// restore verification has any discriminating power — see `try_candidate`'s `readback=`.
unsafe fn witnessed_write(bar0: usize, tag: &str, name: &str, off: usize, val: u32, src: &str) -> u32 {
    let pre = rd(bar0, off);
    wr(bar0, off, val);
    let post = rd(bar0, off);
    serial_println!(
        ":: gen7: {} step name={} off={:06X} wrote={:08X} pre={:08X} post={:08X} src={} ::",
        tag,
        name,
        off,
        val,
        pre,
        post,
        src
    );
    post
}

/// Read the whole battery once (each register twice), record values and per-register
/// variance, and print one `col=` line per register. `label` is the column name (`pre` /
/// `held` for R2; `r3pre` / `mt` / `renfw` / `r3post` for R3).
///
/// ⚠ The `touched=` field on each line is **R2's annotation** — it is the `GT_BATTERY` tuple's
/// fourth element and it means "the R2 wake sequence writes this register". R3 writes none of
/// the seventeen, so under an `r3` tag a `touched=1` line is still a register R3 never wrote,
/// and R3's liveness count is drawn from all seventeen precisely because of that. The field
/// keeps its name across both rungs so a capture analyzer written for Boot D still parses.
unsafe fn read_battery(
    bar0: usize,
    tag: &str,
    label: &str,
    v: &mut [u32; BATTERY_N],
    varied: &mut [bool; BATTERY_N],
) {
    for (i, &(blk, name, off, touched)) in GT_BATTERY.iter().enumerate() {
        let a = rd(bar0, off);
        let b = rd(bar0, off);
        v[i] = a;
        varied[i] = a != b;
        serial_println!(
            ":: gen7: {} col={} blk={} name={} off={:06X} v0={:08X} v1={:08X} cls={} varies={} touched={} ::",
            tag,
            label,
            blk,
            name,
            off,
            a,
            b,
            Cls::of(a).name(),
            if a != b { 1 } else { 0 },
            if touched { 1 } else { 0 }
        );
    }
}

/// GEN7 rung R2 — wake the GT power well, and prove it on the registers that read dark.
///
/// Called from `igpu::init` immediately after `recon`, still BEFORE `bring_up_blt_ring`.
/// This is the ladder's first write. The sequence is the IVB Sync-Flush workaround
/// (draft §3.1 P2 / IVB-V1P3 §1.1.10.9, pp.70-71), which wakes the command streamer as a
/// side effect of draining it — NOT a general forcewake protocol, and the module does not
/// pretend it is one.
///
/// # Safety
/// Same contract as `recon`: `bar0` is a live MMIO mapping of at least `bar0_size` bytes of
/// the IGD's BAR0. R2 writes three GT power-management registers (`INSTPM`, `RCS_WAKE`) and
/// re-parks `INSTPM`; it writes no GGTT entry, no ring register and no display register, and
/// the re-park runs on every exit path.
pub unsafe fn wake(bar0: usize, bar0_size: usize, bus: u8, slot: u8, func: u8) {
    serial_println!(
        ":: gen7: wake begin rung=R2 mode=write writes=3 bdf={}:{}.{} bar0_size={} ladder=GEN7-3D ::",
        bus,
        slot,
        func,
        bar0_size
    );

    // ---- Parachute, identical discipline to R1 ---------------------------------------
    if bar0 == 0 {
        serial_println!(":: gen7: r2 verdict=refused-no-bar0 — BAR0 unpublished; no write attempted ::");
        serial_println!(":: gen7: wake end ::");
        return;
    }
    // The highest offset R2 touches is BCS_UHPTR (0x22134). Refuse the whole rung — BEFORE
    // any write — if the enumerated window does not cover the registers we would read and
    // write, rather than reading/writing into whatever follows the mapping. This mirrors
    // R1's `ggtt` short-BAR0 refusal and it is checked against the size we were GIVEN, not a
    // literal we expect.
    const MAX_OFF: usize = g7regs::BCS_UHPTR + 4;
    if bar0_size < MAX_OFF {
        serial_println!(
            ":: gen7: r2 verdict=refused-bar0-too-small have={} need={} — no write attempted ::",
            bar0_size,
            MAX_OFF
        );
        serial_println!(":: gen7: wake end ::");
        return;
    }

    // Bounded poll budget. A well that never acks is VOID, not a hang: after this many reads
    // of 0x22AC with bits[3:0] still non-zero, the rung gives up, re-parks, and prints
    // `verdict=wake-void`. Matches the iteration order-of-magnitude of `BltRing::submit`.
    const POLL_MAX: u32 = 1_000_000;

    // ---- Before column ----------------------------------------------------------------
    let mut before = [0u32; BATTERY_N];
    let mut before_var = [false; BATTERY_N];
    read_battery(bar0, "wake", "pre", &mut before, &mut before_var);

    // ---- The wake sequence (draft §3.1 P2 / IVB-V1P3 §1.1.10.9 pp.70-71) --------------
    // Every write PINNED against IVB-V1P3 (the doc pinned in g7regs). The 0x00010001 /
    // 0x00010000 form is the upper-16-as-write-enable-mask semantics pinned generically in
    // the same volume ~p.66.
    //
    // Step 1 — INSTPM disable sequence: set data bit 0 (mask bit 16 arms it).
    witnessed_write(bar0, "wake", "INSTPM_disable", g7regs::INSTPM, 0x0001_0001, "IVB-V1P3-1.1.10.9-p70");
    // Step 2 — "Wake up CS but don't do anything".
    witnessed_write(bar0, "wake", "RCS_WAKE_poke", g7regs::RCS_WAKE, 0x0000_0000, "IVB-V1P3-1.1.10.9-p71");

    // Step 3 — poll 0x22AC[3:0] == 0 ("Guarantees render pipe is awake"), bounded.
    let mut iters = 0u32;
    let mut idle;
    loop {
        idle = rd(bar0, g7regs::RENDER_IDLE_POLL);
        if idle & 0xF == 0 {
            break;
        }
        iters += 1;
        if iters >= POLL_MAX {
            break;
        }
    }
    let poll_ack = idle & 0xF == 0;
    serial_println!(
        ":: gen7: wake poll off={:06X} idle={:08X} idle_lo4={:X} iters={} max={} ack={} ::",
        g7regs::RENDER_IDLE_POLL,
        idle,
        idle & 0xF,
        iters,
        POLL_MAX,
        if poll_ack { 1 } else { 0 }
    );

    // ---- Held column — read the SAME battery while the wake is asserted ---------------
    let mut held = [0u32; BATTERY_N];
    let mut held_var = [false; BATTERY_N];
    read_battery(bar0, "wake", "held", &mut held, &mut held_var);

    // ---- Re-park — restore INSTPM = 0x00010000 (clear data bit 0 via mask bit 16) ------
    // This runs on EVERY path that reached here (poll ack OR poll timeout): it is a
    // straight-line write, not inside either branch. It returns INSTPM to the RC6-eligible
    // state firmware left, so the GT may re-enter RC6; no display register is touched and the
    // panel is the Kepler's regardless. The post-read is the reversal witness.
    witnessed_write(bar0, "wake", "INSTPM_repark", g7regs::INSTPM, 0x0001_0000, "IVB-V1P3-1.1.10.9-p71");

    // ---- The transition tally — Wall D: a value WE caused is not a witness -------------
    // A register counts as "was dark" if its pre column was zero-or-ones AND did not vary,
    // and "now live" if its held column is structured OR varied on the read-twice. A
    // dark→live transition on a register we NEVER wrote (touched=false) is the honest
    // signal; the three touched registers are counted in `trans_all` for completeness but
    // never in the untouched liveness count the verdict rests on.
    let mut trans_all = 0u32;
    let mut trans_untouched = 0u32;
    let mut untouched_struct = 0u32;
    let mut untouched_varies = 0u32;
    for (i, &(_blk, _name, _off, touched)) in GT_BATTERY.iter().enumerate() {
        let was_dark = matches!(Cls::of(before[i]), Cls::Zero | Cls::AllOnes) && !before_var[i];
        let now_struct = matches!(Cls::of(held[i]), Cls::Structured);
        let now_live = now_struct || held_var[i];
        if was_dark && now_live {
            trans_all += 1;
            if !touched {
                trans_untouched += 1;
                if now_struct {
                    untouched_struct += 1;
                }
                if held_var[i] {
                    untouched_varies += 1;
                }
            }
        }
    }

    // ---- The rung verdict — named before the probe ran --------------------------------
    //  wake-void      the poll never cleared within POLL_MAX  → a Sync-Flush workaround
    //                 borrowed out of context failing tells us about the borrowing, not the
    //                 GT (draft §3.1 P2). No wake reading is drawn.
    //  gt-woke        poll acked AND the fourteen untouched ring registers show real motion
    //                 (≥2 gone structured, or any read-twice varies) → the GT power well is
    //                 awake and readable, proven on registers that were dark. STOP HERE.
    //  gt-still-dark  poll acked but the untouched battery is unchanged → a real, surprising
    //                 finding: the ack asserted (or 0x22AC was already zero, as a gated
    //                 window reads) yet nothing lit. The forcewake block is elsewhere.
    let verdict = if !poll_ack {
        "wake-void"
    } else if untouched_struct >= 2 || untouched_varies >= 1 {
        "gt-woke"
    } else {
        "gt-still-dark"
    };
    serial_println!(
        ":: gen7: r2 verdict={} trans_all={}/{} trans_untouched={}/14 struct={} varies={} poll_ack={} poll_iters={} rung=R2 wrote=3 reparked=1 ::",
        verdict,
        trans_all,
        BATTERY_N,
        trans_untouched,
        untouched_struct,
        untouched_varies,
        if poll_ack { 1 } else { 0 },
        iters
    );
    serial_println!(
        ":: gen7: r2 next={} note=re-parked-INSTPM=0x00010000-no-display-register-touched ::",
        match verdict {
            "gt-woke" => "R3-ggtt-claim(read-only)-then-R4-ring-then-R5-MI_STORE_DATA_IMM",
            "gt-still-dark" => "R3-forcewake-request/ack-pairs(0x0A188/0x130044,0x1300B0/0x1300B4)",
            _ => "STOP-poll-never-cleared-sync-flush-wa-out-of-context-is-not-a-general-wake",
        }
    );
    serial_println!(":: gen7: wake end ::");
}

// ===================================================================================
// R3 — the forcewake acquire. The rung that goes after the GT power well itself.
// ===================================================================================
//
// ## Why R3 exists, in one paragraph
//
// R2 flew on Boot D (`~/unaos-bench/capture/gr26-bootD/ttyUSB0.log`) and returned
// `:: gen7: r2 verdict=gt-still-dark trans_all=0/17 trans_untouched=0/14 struct=0 varies=0
// poll_ack=1 poll_iters=0 rung=R2 wrote=3 reparked=1 ::`. Read carefully, that line says
// three separate things, and only the first is good news:
//
//  * The write path is real — the rung reached the wire and returned. Nothing hung.
//  * `poll_ack=1 poll_iters=0` is **not** an ack. `0x22AC` read `0x00000000` on the first
//    look, and `idle[3:0] == 0` is the pass condition, so a **power-gated window that
//    returns zero for everything passes this poll on iteration zero**. The R2 verdict block
//    already said so in as many words ("or `0x22AC` was already zero, as a gated window
//    reads"); Boot D is that sentence coming true. R3 must therefore never build a verdict
//    on a zero-valued pass condition again — every ack test below is a **transition** test.
//  * `wake step name=INSTPM_disable wrote=00010001 pre=00000000 post=00000000` — the write
//    did not latch anything readable. On a mask-write register a zero post-read is expected
//    (the mask field reads zero) but bit 0 of the DATA field should have read back `1`.
//    It read `0`. Combined with `trans_untouched=0/14`, the plain reading is: **the whole
//    0x2xxxx GT register block is behind a closed power well, and the Sync-Flush workaround
//    cannot open it because draining a command streamer is not the same act as powering
//    one.** That is the wall the draft's §4.1 named, standing exactly where it said it
//    would.
//
// So R3 stops borrowing a workaround and goes at the documented mechanism: a forcewake
// **request** register and its **ack** partner, held while the battery is read.
//
// ## Provenance, and the clean-room line this rung does not cross
//
// The draft's §0 is binding: Intel PRMs are a legal pinning target, **Linux `i915` and Mesa
// `i965` driver source are off-limits and are not a source for anything in this ladder.**
// Every offset R3 touches was already in `g7regs` before this rung existed, pinned by
// document, volume and page **on Broadwell or on Cherryview/Braswell** — because Intel never
// published the Gen7 forcewake block (R0.1 searched all sixteen IVB volumes; the only hit is
// the register-less "Force Wakeup bit" prose). No offset in this rung comes from driver
// source, and none is claimed as an Ivy Bridge fact. They are hypotheses, and §0 says
// exactly what a hypothesis is for: *to be tested on our own silicon*, which is what a rung
// is. The wire says `pin=BDW-ONLY` / `pin=CHV-ONLY` on every line so no reader can mistake
// the class.
//
// ## The two candidates, and why both fly in one boot
//
//  * **A — MT.** Request `FORCE_WAKE` `0x0A188`, ack `GTSP1 0x130044[15:0]`
//    ("GT programs this field with the multiple force wake status") — [BDW-ONLY],
//    IHD-OS-BDW-Vol 2c-11.15 pp.493 / 703. Mask-write form: `0x00010001` requests thread 0,
//    `0x00010000` releases it. This is the pair whose *ack partner is not in the 0xA18x
//    block*, which is the specific fact R0.1 established and the draft as first written got
//    wrong.
//  * **B — per-well RENFW.** Request `0x1300B0`, ack `0x1300B4` — [CHV-ONLY],
//    IHD-OS-CHV-BSW-Vol 2c-10.15 pp.1078/1077, "Driver must poll on the corresponding bit to
//    confirm that the well has woken". A different shape (per-power-well rather than
//    per-thread) at a different offset.
//
// The draft's §5 rule is "one variable per boot, **with an A/B fallback where a single
// binary question is open**". The question is binary ("does either published pair decode on
// Gen7?") and B runs **only if A did not wake the GT** — where "wake" means an ack transition
// AND real battery motion — each with its own hold column and its own release.
//
// ⚠ How far that carries, exactly (review, GR26). It makes the VERDICT unambiguous: `woke_by`
// and `motion_by` both test candidate A first, so any motion A produced is attributed to A.
// It does **not** make every per-candidate line clean evidence, and the earlier wording here
// ("a battery that lights under B alone is B's") overstated it. The gap is the
// motion-without-ack shape: A lights the battery, `a_woke` is false because no ack asserted,
// B runs anyway, and B's `cand=renfw battery` line is then read on a GT A may already have
// lit. Closing it would mean skipping B on motion alone — spending the boot's second
// candidate on precisely the reading that most needs one — so it is documented instead, and
// the rule for a capture reader is: take the `mt` lines first.
//
// ## Wall D for R3 — three ways this rung refuses to fool itself
//
// 1. **Every ack test is a transition test with a stability precondition.** An ack counts
//    only if the register's entry column was `0x0000` in the watched field AND read twice
//    identically, and the held column is non-zero in that field. Boot D proved why: a
//    zero-pass condition passed against a dead well on iteration zero.
// 2. **The liveness evidence is the battery R3 never writes.** All seventeen battery
//    registers are untouched by R3 (it writes only `0x0A188` / `0x1300B0`, neither of which
//    is in the battery) — so unlike R2, there is no touched/untouched split to police here
//    and the whole battery is an honest witness. `gt-woke` needs REAL motion in it (≥2
//    registers gone structured, or any read-twice `varies`), the same threshold R1 and R2
//    used, **and** an ack transition. Motion without an ack is not thrown away — it gets its
//    own verdict (`gt-woke-noack`), because "the write worked and the ack register is not
//    where we think" is a finding, not a null.
// 3. **`gt-live-already` dominates the verdict.** If the entry battery — measured before any
//    write — is already live, a transition-based wake claim is unreadable in principle, and
//    the rung says so instead of counting motion it cannot attribute. That arm also happens to
//    be the draft's "CONFIRMED (G1 false)" outcome. ⚠ Stated precisely (review, GR26): this is
//    a verdict rule, **not a guard that skips the acquires**. Both candidates still run under
//    `pre_live`, deliberately — on an already-live GT the ack registers' behaviour is the open
//    question R3 exists to answer, and refusing to write would discard it while making nothing
//    safer. Earlier wording here said "checked first", which read as a skip.
//
// ## Reversibility — the rule this rung is most exposed to, and how it is verified
//
// Each candidate is released **in the same rung, on every exit path**, and the release is
// double-form because the register's write semantics are themselves a hypothesis: first the
// mask-form release (`0x00010000`, which clears data bit 0 on a mask register and is what
// the BDW pin documents), then a plain write of **the exact dword read at entry** (a no-op
// on a mask register, since a zero mask modifies nothing; an exact restore on a plain
// register). Whichever semantics the part really has, the register ends at its entry value —
// and the rung does not take that on faith: it re-reads and prints `restored=` per candidate
// and a rung-wide `restore=clean|dirty`, with a `dirty` result named on the verdict line
// rather than buried.
//
// ⚠ AND THE RE-READ IS ITSELF SUSPECT ON THIS PART (review, GR26) — the honesty this
// paragraph claims has to survive Wall D pointed at it. `restored=` is
// `req_post == req_pre && ack_post == ack_pre`, and Boot D predicts **all four of those
// dwords read `0x00000000`** on this silicon: every 0xA18x and every 0x13xxxx offset read
// zero. A `0 == 0` compare cannot fail, so on the expected readings `restored=1` would be
// printed by a rung that released nothing at all — a witness that cannot fail, in a file that
// convicts other people's instruments for exactly that. It is not fixable by testing
// something else (there is nothing else to read), so the rung measures whether the check had
// any power and says so: `readback=` (did the acquire write change the request register's
// readback?), `ack_moved=`, and `evidence=` per candidate, plus `restore_evidence=real|blind`
// on the verdict line. `restore=clean restore_evidence=blind` is the honest form of "as far
// as anything readable on this part can tell" — and it is the outcome to EXPECT on Boot B.
// The release itself is unaffected: it is correct under either semantics and does not depend
// on the register being readable; only the claim about it is now bounded.
// The verdict line also carries `frame=quiet|moved` separately from `restore=`, because a
// change in the six frame registers R3 never writes is a statement about the GT, not about
// our footprint, and folding it into `dirty` made a live status register (`GTFIFOCTL`) able
// to report a perfectly-restored rung as dirty.
//
// A candidate whose request register is **already non-zero at entry** is
// skipped without a write (`skipped=req-preheld`): something else is holding it and this rung
// will not stomp another owner's state.
//
// No clock is reprogrammed, no RC6/RPS policy register is touched, no voltage or frequency
// request is made, and nothing in a display block is written — the panel is the Kepler's
// regardless (`igpu.rs` ~778, Boot AS).

/// What R3 concluded about the GT power well, handed to R4 so it branches on a real verdict
/// rather than re-deriving one. R4's entire safety argument is that it does **not** write GGTT
/// PTEs behind a closed well, and `reachable()` is the single gate on the one write path in the
/// rung: `Dark` — the outcome Boot D's `gt-still-dark` makes most likely — routes R4 to a
/// read-only recon that still produces a verdict.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GtWake {
    /// R3 saw the well open: an ack transition AND real battery motion (`gt-woke`).
    Woke,
    /// The battery lit under a hold whose ack never asserted (`gt-woke-noack`): the GT block is
    /// reachable, the ack register is simply not where we looked.
    WokeNoAck,
    /// The battery was already live before any write (`gt-live-already`, G1 false on this part):
    /// the well was never closed.
    LiveAlready,
    /// No wake — `fw-no-ack` / `fw-acked-gt-dark` / `both-req-preheld` / any R3 refusal. The GT
    /// block is gated (or its state is unknown), and R4 must not write behind it.
    Dark,
}

impl GtWake {
    /// May R4 attempt its one reversible GGTT write? Only on positive R3 evidence that the GT
    /// block is reachable. `Dark` is a hard no — that is the structural promise that a closed
    /// well is never written behind, expressed as a single boolean the write path is gated on.
    fn reachable(self) -> bool {
        matches!(self, GtWake::Woke | GtWake::WokeNoAck | GtWake::LiveAlready)
    }
    fn name(self) -> &'static str {
        match self {
            GtWake::Woke => "gt-woke",
            GtWake::WokeNoAck => "gt-woke-noack",
            GtWake::LiveAlready => "gt-live-already",
            GtWake::Dark => "dark",
        }
    }
}

/// The always-on / power-frame registers R3 reads in **every** column. None of these is in
/// `GT_BATTERY`, and R3 writes exactly two of them (the two request registers), which is why
/// each row carries whether it is a written row.
///
/// `HYP_GTFIFOCTL` earns its place from metal: it was the only structured read in R1's
/// 25-register probe on Boot D (`0x0000003F`, stable), so it is this machine's nearest thing
/// to an always-on GT witness. Its *value* is not decoded — only its steadiness and any
/// change across an acquire.
const FW_FRAME: &[(&str, usize, &str)] = &[
    ("FORCEWAKE_MT_REQ", g7regs::HYP_FORCEWAKE_MT, "BDW-ONLY-p493"),
    ("FORCEWAKE_MT_ACK", g7regs::HYP_FORCEWAKE_MT_ACK, "BDW-ONLY-p703"),
    ("RENFW_REQ", g7regs::HYP_RENFW_REQ, "CHV-ONLY-p1078"),
    ("RENFW_ACK", g7regs::HYP_RENFW_ACK, "CHV-ONLY-p1077"),
    ("GTFORCEAWAKE", g7regs::HYP_GTFORCEAWAKE, "BDW-ONLY-p656"),
    ("GTLC_PW_STAT", g7regs::HYP_GTLC_PW_STAT, "CHV-ONLY-p487"),
    ("MISC_CTRL0", g7regs::HYP_MISC_CTRL0, "BDW-ONLY-p605"),
    ("GTFIFOCTL", g7regs::HYP_GTFIFOCTL, "CHV-ONLY-p451/METAL-BootD"),
];

const FRAME_N: usize = 8;

/// Read the power frame once (each register twice) and print one line per register.
unsafe fn read_frame(bar0: usize, label: &str, v: &mut [u32; FRAME_N], varied: &mut [bool; FRAME_N]) {
    for (i, &(name, off, pin)) in FW_FRAME.iter().enumerate() {
        let a = rd(bar0, off);
        let b = rd(bar0, off);
        v[i] = a;
        varied[i] = a != b;
        serial_println!(
            ":: gen7: r3 frame col={} name={} off={:06X} v0={:08X} v1={:08X} cls={} varies={} pin={} ::",
            label,
            name,
            off,
            a,
            b,
            Cls::of(a).name(),
            if a != b { 1 } else { 0 },
            pin
        );
    }
}

/// How a candidate acquire ended. Every outcome is named here, before the rung runs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Acq {
    /// The request register was already non-zero at entry — another owner holds it. No write
    /// was made, and no reading is drawn from this candidate.
    SkippedPreheld,
    /// Written, and the ack field made a real dark→set transition.
    AckTransition,
    /// Written, and the ack field never left its entry value within the poll budget.
    NoAck,
    /// Written, but the ack register's ENTRY column was already non-zero or unstable, so a
    /// transition is not readable on it. The write still happened and the battery column is
    /// still valid — this arm exists so an unreadable ack is never scored as an ack.
    AckUnreadable,
}

impl Acq {
    fn name(self) -> &'static str {
        match self {
            Acq::SkippedPreheld => "skipped-req-preheld",
            Acq::AckTransition => "ack-transition",
            Acq::NoAck => "no-ack",
            Acq::AckUnreadable => "ack-unreadable",
        }
    }
}

/// Liveness reading of one battery column, measured against the entry column.
struct Motion {
    /// Registers dark at entry that read structured in this column.
    gone_struct: u32,
    /// Registers that varied between their two reads in this column.
    varies: u32,
}

impl Motion {
    /// The alive threshold, identical to R1's and R2's so the three rungs cannot disagree
    /// about what "live" means: real motion, or at least two registers gone structured.
    /// One stray latched bit is not a wake.
    fn live(&self) -> bool {
        self.gone_struct >= 2 || self.varies >= 1
    }
}

/// Compare a held battery column against the entry column.
fn motion(before: &[u32; BATTERY_N], before_var: &[bool; BATTERY_N], held: &[u32; BATTERY_N], held_var: &[bool; BATTERY_N]) -> Motion {
    let mut m = Motion { gone_struct: 0, varies: 0 };
    for i in 0..BATTERY_N {
        let was_dark = matches!(Cls::of(before[i]), Cls::Zero | Cls::AllOnes) && !before_var[i];
        if was_dark && matches!(Cls::of(held[i]), Cls::Structured) {
            m.gone_struct += 1;
        }
        if held_var[i] {
            m.varies += 1;
        }
    }
    m
}

/// One forcewake candidate: acquire, poll its ack, read the battery under the hold, release,
/// verify the release. Returns `(outcome, motion, restored, restore_evidence)`.
///
/// `restore_evidence` is the fourth element and it is the review fix that matters most in this
/// function: it reports whether the `restored` verification could have failed at all on this
/// part. See the block above the `release` line.
///
/// `ack_mask` is the field watched on the ack register. It is the low sixteen bits for both
/// candidates: BDW pins GTSP1`[15:0]` as the multiple-force-wake status field, and the CHV
/// per-well pair is documented as "poll on the corresponding bit" without pinning WHICH bit
/// on this part — so the honest watch is the whole documented field, with the strict bit-0
/// reading printed alongside it. A wider field cannot manufacture an ack here, because the
/// test is a transition from a **stable zero** entry column, not a non-zero compare.
#[allow(clippy::too_many_arguments)]
unsafe fn try_candidate(
    bar0: usize,
    cand: &str,
    req_off: usize,
    ack_off: usize,
    ack_mask: u32,
    src: &str,
    poll_max: u32,
    before: &[u32; BATTERY_N],
    before_var: &[bool; BATTERY_N],
) -> (Acq, Motion, bool, bool) {
    let no_motion = Motion { gone_struct: 0, varies: 0 };

    // Entry images. `req_pre` is what the release must restore the request register to —
    // read once for the restore target and once more for stability, because a restore target
    // that is itself unstable is not a restore target.
    let req_pre = rd(bar0, req_off);
    let req_pre2 = rd(bar0, req_off);
    let ack_pre = rd(bar0, ack_off);
    let ack_pre2 = rd(bar0, ack_off);
    let ack_readable = (ack_pre & ack_mask) == 0 && ack_pre == ack_pre2;
    serial_println!(
        ":: gen7: r3 cand={} entry req_off={:06X} req_pre={:08X} req_pre2={:08X} ack_off={:06X} ack_pre={:08X} ack_pre2={:08X} ack_mask={:08X} ack_readable={} src={} ::",
        cand,
        req_off,
        req_pre,
        req_pre2,
        ack_off,
        ack_pre,
        ack_pre2,
        ack_mask,
        if ack_readable { 1 } else { 0 },
        src
    );

    // Refusal: someone already holds this request register. Do not write over another
    // owner's state — read, report, return.
    if req_pre != 0 || req_pre2 != 0 {
        serial_println!(
            ":: gen7: r3 cand={} outcome={} wrote=0 note=request-register-non-zero-at-entry-not-ours-to-clear ::",
            cand,
            Acq::SkippedPreheld.name()
        );
        // `evidence=true`: a skipped candidate makes no claim about a RELEASE, only the claim
        // "nothing was written", and that one is directly established by the two entry reads
        // printed above. It is not the vacuous `0 == 0` case the release check has to guard.
        return (Acq::SkippedPreheld, no_motion, true, true);
    }

    // ---- Acquire ------------------------------------------------------------------------
    // Mask-write form: upper 16 bits arm the corresponding data bits (IVB-V1P3 ~p.66 for the
    // generic semantics; BDW p.493 for this register's mask field). Requesting data bit 0.
    // The post-read is KEPT (review, GR26): whether this register reads back its own write is
    // what decides whether the release verification below can falsify anything at all. See
    // `readback=` on the release line.
    let acq_post = witnessed_write(bar0, "r3", "FW_REQUEST", req_off, 0x0001_0001, src);

    // ---- Poll the ack, bounded ------------------------------------------------------------
    // The pass condition is `(ack & mask) != 0` — a transition away from the stable zero
    // entry column. This is the direct correction of the R2/Boot-D defect, where the pass
    // condition was `== 0` and a dead window satisfied it on iteration zero.
    //
    // ⚠ COST, and why it is on the wire (review, GR26). The budget is an ITERATION count, and
    // an iteration count is not a time. The expected-negative outcome runs this loop to expiry
    // on BOTH candidates, so R3's expected boot cost is `2 * poll_max` uncached MMIO reads —
    // and until it flies, nobody knows what that is in milliseconds. `now_cycles()` (rdtsc,
    // invariant on this part, and it advances regardless of EFLAGS.IF) brackets the loop, so
    // `cyc=` converts `iters=` into a real number the first time this rung runs. The whole
    // `igpu::init` span is already inside the boot-pace `G_IGPU` bucket (`arch/x86_64/pci.rs`),
    // so the cost is double-booked deliberately: once as a phase, once per poll.
    let t0 = crate::arch::now_cycles();
    let mut iters = 0u32;
    let mut ack;
    loop {
        ack = rd(bar0, ack_off);
        if (ack & ack_mask) != 0 {
            break;
        }
        iters += 1;
        if iters >= poll_max {
            break;
        }
    }
    let cyc = crate::arch::now_cycles().wrapping_sub(t0);
    let ack_set = (ack & ack_mask) != 0;
    serial_println!(
        ":: gen7: r3 cand={} poll ack_off={:06X} ack={:08X} masked={:08X} bit0={} iters={} max={} set={} cyc={} ::",
        cand,
        ack_off,
        ack,
        ack & ack_mask,
        ack & 1,
        iters,
        poll_max,
        if ack_set { 1 } else { 0 },
        cyc
    );

    // ---- The held column -------------------------------------------------------------------
    // Read whether or not the ack asserted: an ack register in the wrong place would not stop
    // the request register from having woken the well, and that case has its own verdict.
    let mut held = [0u32; BATTERY_N];
    let mut held_var = [false; BATTERY_N];
    read_battery(bar0, "r3", cand, &mut held, &mut held_var);
    let m = motion(before, before_var, &held, &held_var);
    serial_println!(
        ":: gen7: r3 cand={} battery gone_struct={} varies={} of={} live={} ::",
        cand,
        m.gone_struct,
        m.varies,
        BATTERY_N,
        if m.live() { 1 } else { 0 }
    );

    // ---- Release, in two forms, on every path ---------------------------------------------
    // Form 1 — mask release: clears data bit 0 on a mask-write register (the documented
    // semantics). Form 2 — exact restore: writes back the dword read at entry, which is a
    // no-op on a mask register (mask=0 modifies nothing) and an exact restore on a plain one.
    // Together they return the register to its entry value under EITHER semantics, which
    // matters because which semantics this part implements is precisely what is unpinned.
    witnessed_write(bar0, "r3", "FW_RELEASE_MASK", req_off, 0x0001_0000, src);
    witnessed_write(bar0, "r3", "FW_RELEASE_EXACT", req_off, req_pre, src);

    // ---- Verify the release ----------------------------------------------------------------
    // Restore is not assumed. `restored` is true only if the request register reads back its
    // entry dword AND the ack field has returned to its entry value (an ack still asserted
    // after release means the well is being held by something we cannot see, and that is a
    // finding the rung must not swallow).
    let req_post = rd(bar0, req_off);
    let ack_post = rd(bar0, ack_off);
    let req_ok = req_post == req_pre;
    let ack_ok = (ack_post & ack_mask) == (ack_pre & ack_mask);
    let restored = req_ok && ack_ok;

    // ⚠ WALL D, APPLIED TO THIS RUNG'S OWN SAFETY WITNESS (review, GR26). `restored=1` above
    // is `req_post == req_pre` AND `ack_post == ack_pre` — and on the readings Boot D
    // predicts for this part, **every one of those four dwords is `0x00000000`**. A test of
    // `0 == 0` against `0 == 0` cannot fail, so `restored=1` would be printed by a rung that
    // released nothing, on a part where the register never reads back anything. That is
    // precisely the class of defect this module names elsewhere — a witness that cannot fail —
    // and the safety argument in this file's header leans on it ("the rung does not take that
    // on faith: it re-reads").
    //
    // So the rung measures whether the check HAD any discriminating power, and says so:
    //   * `req_readback` — did the acquire write change the request register's readback at
    //     all? If `acq_post == req_pre`, the register did not read back our own write, so the
    //     later `req_post == req_pre` compare is uninformative about the release.
    //   * `ack_moved` — did the ack field ever leave its entry value? If not, the `ack_ok`
    //     compare is uninformative for the same reason.
    // `evidence=1` means at least one of the two halves could actually have failed.
    // `evidence=0` means `restored=1` is a statement about a register pair that never moved:
    // still the best available reading, but NOT proof that a hold was released, and the wire
    // says so rather than letting `clean` be read as proof.
    //
    // This does not weaken the release itself. The release remains correct under either
    // semantics — the mask form clears data bit 0 on a mask register, and the exact-dword form
    // runs LAST and restores a plain register — and neither depends on the register being
    // readable. What changes is only the honesty of the claim about it.
    let req_readback = acq_post != req_pre;
    let ack_moved = (ack & ack_mask) != (ack_pre & ack_mask);
    let evidence = req_readback || ack_moved;
    serial_println!(
        ":: gen7: r3 cand={} release req_post={:08X} req_pre={:08X} req_ok={} ack_post={:08X} ack_pre={:08X} ack_ok={} restored={} readback={} ack_moved={} evidence={} ::",
        cand,
        req_post,
        req_pre,
        if req_ok { 1 } else { 0 },
        ack_post,
        ack_pre,
        if ack_ok { 1 } else { 0 },
        if restored { 1 } else { 0 },
        if req_readback { 1 } else { 0 },
        if ack_moved { 1 } else { 0 },
        if evidence { 1 } else { 0 }
    );

    let outcome = if !ack_readable {
        Acq::AckUnreadable
    } else if ack_set {
        Acq::AckTransition
    } else {
        Acq::NoAck
    };
    serial_println!(
        ":: gen7: r3 cand={} outcome={} wrote=3 released=1 restored={} evidence={} ::",
        cand,
        outcome.name(),
        if restored { 1 } else { 0 },
        if evidence { 1 } else { 0 }
    );
    (outcome, m, restored, evidence)
}

/// GEN7 rung R3 — acquire forcewake on the published request/ack pairs and prove the GT
/// power well opened, on registers the rung never writes.
///
/// Called from `igpu::init` immediately after `wake` (R2) and still BEFORE
/// `bring_up_blt_ring`. R2 re-parks `INSTPM` on every exit path, so R3 starts from the state
/// firmware left.
///
/// # Safety
/// Same contract as `recon` and `wake`: `bar0` is a live MMIO mapping of at least `bar0_size`
/// bytes of the IGD's BAR0. R3 writes at most two registers — the forcewake **request**
/// registers `0x0A188` and `0x1300B0` — one candidate at a time, releasing each in-rung and
/// verifying the release against the entry value. It writes no ring register, no GGTT entry
/// and no display register.
///
/// Returns the wake reading as a `GtWake` so R4 (`claim`) branches on R3's real verdict rather
/// than re-deriving one. The two parachutes return `GtWake::Dark` — a refused rung has, by
/// definition, no evidence the well is open, and R4's write path must stay closed on it.
pub unsafe fn forcewake(bar0: usize, bar0_size: usize, bus: u8, slot: u8, func: u8) -> GtWake {
    serial_println!(
        ":: gen7: r3 begin rung=R3 mode=write cands=2 bdf={}:{}.{} bar0_size={} ladder=GEN7-3D ::",
        bus,
        slot,
        func,
        bar0_size
    );

    // ---- Parachutes, identical discipline to R1/R2 -------------------------------------
    if bar0 == 0 {
        serial_println!(":: gen7: r3 verdict=refused-no-bar0 — BAR0 unpublished; no write attempted ::");
        serial_println!(":: gen7: r3 end ::");
        return GtWake::Dark;
    }
    // The highest offset R3 touches is RENFW_ACK (0x1300B4); the battery reaches BCS_UHPTR
    // (0x22134). Checked against the size we were GIVEN, before any write, exactly as R2's
    // guard is — and against the true maximum of the two, not the one that happens to be
    // larger on the machine we have.
    const MAX_OFF: usize = g7regs::HYP_RENFW_ACK + 4;
    if bar0_size < MAX_OFF {
        serial_println!(
            ":: gen7: r3 verdict=refused-bar0-too-small have={} need={} — no write attempted ::",
            bar0_size,
            MAX_OFF
        );
        serial_println!(":: gen7: r3 end ::");
        return GtWake::Dark;
    }

    // Bounded ack budget. Deliberately smaller than R2's `POLL_MAX`: R3 may run this poll
    // TWICE and the expected-negative outcome is exactly the one that runs it to expiry, so
    // an over-large budget buys nothing and spends boot time on a wall we already suspect.
    // `iters=` is on the wire, so `no-ack` is always readable as "no ack within THIS budget"
    // rather than as a claim about eternity.
    const ACK_POLL_MAX: u32 = 200_000;

    // ---- The entry columns ---------------------------------------------------------------
    let mut frame_pre = [0u32; FRAME_N];
    let mut frame_pre_var = [false; FRAME_N];
    read_frame(bar0, "pre", &mut frame_pre, &mut frame_pre_var);

    let mut before = [0u32; BATTERY_N];
    let mut before_var = [false; BATTERY_N];
    read_battery(bar0, "r3", "r3pre", &mut before, &mut before_var);

    // Is the battery ALREADY live? If so, no transition-based wake claim is readable, and the
    // rung says that rather than counting motion it cannot attribute. This is the draft's
    // "CONFIRMED (G1 false)" arm.
    //
    // ⚠ PRECISION FIX (review, GR26). The first cut of this comment said the arm "must be
    // tested BEFORE any acquire", and the header block said `gt-live-already` "is checked
    // first" — both of which read as a GUARD that skips the acquires. There is no such guard
    // and there should not be one. What is true, and all that is claimed here: the entry
    // battery is MEASURED before any write (that is what makes it an entry column at all), and
    // `pre_live` DOMINATES the verdict ladder below, above even `gt-woke`. The two acquires
    // still run, deliberately — on an already-live GT the ack registers' behaviour is exactly
    // the open question R3 exists to answer, and refusing to write would throw that away while
    // making the rung no safer. What `pre_live` buys is that no WAKE is claimed from motion
    // that was already there.
    let pre_live = {
        let mut s = 0u32;
        let mut v = 0u32;
        for i in 0..BATTERY_N {
            if matches!(Cls::of(before[i]), Cls::Structured) {
                s += 1;
            }
            if before_var[i] {
                v += 1;
            }
        }
        serial_println!(
            ":: gen7: r3 entry-battery structured={} varies={} of={} ::",
            s,
            v,
            BATTERY_N
        );
        s >= 2 || v >= 1
    };

    // ---- Candidate A — the MT pair (BDW-pinned offsets) ----------------------------------
    let (out_a, m_a, rest_a, ev_a) = try_candidate(
        bar0,
        "mt",
        g7regs::HYP_FORCEWAKE_MT,
        g7regs::HYP_FORCEWAKE_MT_ACK,
        0x0000_FFFF,
        "BDW-2c-11.15-p493/p703",
        ACK_POLL_MAX,
        &before,
        &before_var,
    );

    // ---- Candidate B — the per-well RENFW pair (CHV-pinned offsets) ----------------------
    // Runs only if A did not WAKE the GT — `a_woke` = an ack transition AND real battery
    // motion (draft §5: one variable per boot, A/B fallback on a binary question).
    //
    // ⚠ ATTRIBUTION, STATED EXACTLY (review, GR26). The looser claim — "a lit battery is never
    // ambiguous between the two acquires" — is not what this guard delivers, and the gap is
    // worth naming because it is a real capture-reading trap. `a_woke` requires an ACK; so if
    // A produced motion with NO ack (the `gt-woke-noack` shape), B still runs, and B's held
    // column is then read on a GT that A may already have lit. The VERDICT is safe from it —
    // `woke_by` and `motion_by` both test `m_a` first, so any motion A produced is attributed
    // to A and never to B — but B's own `cand=renfw battery` line in that one case is not
    // clean evidence for B, and a reader must take the `mt` line first. Fixing this by
    // skipping B on motion-alone would cost the boot its second candidate on exactly the
    // reading that most needs one, so the trap is documented rather than closed.
    let a_woke = out_a == Acq::AckTransition && m_a.live();
    let (out_b, m_b, rest_b, ev_b, b_ran) = if a_woke {
        serial_println!(":: gen7: r3 cand=renfw SKIPPED=mt-already-woke-gt note=one-variable-per-boot ::");
        (Acq::NoAck, Motion { gone_struct: 0, varies: 0 }, true, true, false)
    } else {
        let (o, m, r, e) = try_candidate(
            bar0,
            "renfw",
            g7regs::HYP_RENFW_REQ,
            g7regs::HYP_RENFW_ACK,
            0x0000_FFFF,
            "CHV-2c-10.15-p1078/p1077",
            ACK_POLL_MAX,
            &before,
            &before_var,
        );
        (o, m, r, e, true)
    };

    // ---- The exit columns — the reversal witness for the whole rung -----------------------
    let mut frame_post = [0u32; FRAME_N];
    let mut frame_post_var = [false; FRAME_N];
    read_frame(bar0, "post", &mut frame_post, &mut frame_post_var);

    let mut after = [0u32; BATTERY_N];
    let mut after_var = [false; BATTERY_N];
    read_battery(bar0, "r3", "r3post", &mut after, &mut after_var);

    // Frame-level restore check: every frame register back at its entry dword. Mismatches are
    // printed individually — a silent count would hide WHICH register we left moved, which is
    // the only part of a dirty exit anyone can act on.
    let mut frame_moved = 0u32;
    for (i, &(name, off, _pin)) in FW_FRAME.iter().enumerate() {
        if frame_post[i] != frame_pre[i] {
            frame_moved += 1;
            serial_println!(
                ":: gen7: r3 restore-delta name={} off={:06X} pre={:08X} post={:08X} ::",
                name,
                off,
                frame_pre[i],
                frame_post[i]
            );
        }
    }
    let mut battery_moved = 0u32;
    for i in 0..BATTERY_N {
        if after[i] != before[i] {
            battery_moved += 1;
        }
    }
    // `GTLC_PW_STAT` bit 1 is CHV's ALLOWWAKEERR: "when access to media or render is observed
    // when ALLOWWAKE=0, the ALLOWWAKERR bit will be set". If that latch appears on this part
    // it is a direct statement that our GT reads hit a sleeping well — which is the ladder's
    // central question. Printed as a delta, never decoded further; on Boot D the register read
    // `0x00000000`, so its silence is as much a reading as its noise would be.
    //
    // The index is looked up BY NAME rather than written as a literal: a hand-counted index
    // into `FW_FRAME` is a silent-miscount waiting for the next row to be inserted, and a
    // witness that reads the wrong register is the Wall D defect in its purest form. If the
    // row ever disappears, the bits report as zero and the `frame` lines still carry the raw
    // dwords, so the reading degrades to "not measured" rather than to a wrong number.
    let pw_idx = FW_FRAME.iter().position(|r| r.0 == "GTLC_PW_STAT");
    let (pwerr_pre, pwerr_post) = match pw_idx {
        Some(i) => ((frame_pre[i] >> 1) & 1, (frame_post[i] >> 1) & 1),
        None => (0, 0),
    };
    // ⚠ CONFLATION FIX (review, GR26). `restore_clean` was `rest_a && rest_b && frame_moved
    // == 0`, which folded two different statements into one word on the verdict line:
    //   (a) "the two registers R3 WROTE are back at their entry dwords" — a restore claim, and
    //   (b) "none of the six frame registers R3 never wrote changed value" — a statement about
    //       the GT's own state, not about our footprint.
    // Six of the eight frame rows are pure status (`GTFIFOCTL`, `GTLC_PW_STAT`, the two ACKs,
    // `GTFORCEAWAKE`, `MISC_CTRL0`). `GTFIFOCTL` is the ONE live register on this part
    // (`0x0000003F`, Boot D) and a FIFO-state register is exactly the kind that legitimately
    // moves — so under the old predicate a perfectly-restored rung would print `restore=dirty`
    // and read as "R3 left silicon modified". That is a witness crying wolf on the single
    // field Peter will look at first.
    //
    // So the two readings are now separate and BOTH on the verdict line: `restore=` is the
    // written-register claim (`rest_a && rest_b`, each already verified per candidate against
    // its entry dword AND its ack field), and `frame=` is the unwritten-neighbourhood reading.
    // Nothing is lost — including the case that argues for keeping the frame in view at all,
    // a write to `0x0A188` aliasing into `MISC_CTRL0` at `0x0A180`: that still surfaces as
    // `frame=moved` on the verdict line with a named `restore-delta` line under it.
    let restore_clean = rest_a && rest_b;
    let restore_evidence = ev_a && ev_b;
    serial_println!(
        ":: gen7: r3 restore frame_moved={}/{} battery_moved={}/{} cand_mt_restored={} cand_renfw_restored={} mt_evidence={} renfw_evidence={} pwerr_pre={} pwerr_post={} clean={} ::",
        frame_moved,
        FRAME_N,
        battery_moved,
        BATTERY_N,
        if rest_a { 1 } else { 0 },
        if rest_b { 1 } else { 0 },
        if ev_a { 1 } else { 0 },
        if ev_b { 1 } else { 0 },
        pwerr_pre,
        pwerr_post,
        if restore_clean { 1 } else { 0 }
    );

    // ---- The rung verdict — every arm named in source before the rung ran ------------------
    //  refused-no-bar0 / refused-bar0-too-small  parachutes, above; no write made.
    //  both-req-preheld   both request registers were non-zero at entry → nothing written,
    //                     nothing claimed; somebody else owns the block.
    //  gt-live-already    the entry battery was already live → a transition-based wake claim
    //                     is unreadable in principle. G1 is FALSE and that is a clean result;
    //                     every later rung still holds forcewake, because not holding it is a
    //                     silent-corruption class rather than a speed choice.
    //  gt-woke            an ack made a real dark→set transition AND the untouched battery
    //                     lit under that same hold. **The GT power well opened.** `by=` names
    //                     which published pair did it.
    //  gt-woke-noack      the battery lit under a hold whose ack never asserted → the request
    //                     register works and the ack register is not where we think. A
    //                     finding, and a strictly better one than a null.
    //  fw-acked-gt-dark   an ack transitioned but no battery column lit → the well
    //                     acknowledges and the ring block is gated by something else.
    //  fw-no-ack          no ack transition, no motion, both candidates → neither published
    //                     forcewake pair decodes on this part. With R2's `gt-still-dark` this
    //                     is the decisive negative the draft's §4.1 asked for.
    let woke_by = if out_a == Acq::AckTransition && m_a.live() {
        "mt"
    } else if b_ran && out_b == Acq::AckTransition && m_b.live() {
        "renfw"
    } else {
        "none"
    };
    let motion_by = if m_a.live() {
        "mt"
    } else if b_ran && m_b.live() {
        "renfw"
    } else {
        "none"
    };
    let any_ack = out_a == Acq::AckTransition || (b_ran && out_b == Acq::AckTransition);
    let both_preheld = out_a == Acq::SkippedPreheld && (!b_ran || out_b == Acq::SkippedPreheld);

    let verdict = if both_preheld {
        "both-req-preheld"
    } else if pre_live {
        "gt-live-already"
    } else if woke_by != "none" {
        "gt-woke"
    } else if motion_by != "none" {
        "gt-woke-noack"
    } else if any_ack {
        "fw-acked-gt-dark"
    } else {
        "fw-no-ack"
    };

    // ⚠ `renfw_outcome=` when B never ran (review, GR26). The skip arm above seeds `out_b`
    // with `Acq::NoAck` purely as a placeholder, so the old line printed
    // `renfw_ran=0 renfw_outcome=no-ack` — a real-looking outcome for a candidate that was
    // never attempted, and `no-ack` is the very token this rung's decisive negative rests on.
    // A capture analyzer keying on `renfw_outcome=` would have counted it. It now prints
    // `not-run`, which is not in `Acq` (nothing ran, so no acquire outcome exists to name).
    let out_b_name = if b_ran { out_b.name() } else { "not-run" };
    serial_println!(
        ":: gen7: r3 verdict={} by={} mt_outcome={} mt_struct={} mt_varies={} renfw_ran={} renfw_outcome={} renfw_struct={} renfw_varies={} pre_live={} restore={} restore_evidence={} frame={} rung=R3 ::",
        verdict,
        woke_by,
        out_a.name(),
        m_a.gone_struct,
        m_a.varies,
        if b_ran { 1 } else { 0 },
        out_b_name,
        m_b.gone_struct,
        m_b.varies,
        if pre_live { 1 } else { 0 },
        if restore_clean { "clean" } else { "dirty" },
        if restore_evidence { "real" } else { "blind" },
        if frame_moved == 0 { "quiet" } else { "moved" }
    );
    serial_println!(
        ":: gen7: r3 next={} note=forcewake-requests-released-and-verified-no-display-register-touched ::",
        match verdict {
            "gt-woke" => "R3b-hold-forcewake-around-the-R2-sync-flush-then-R4-ggtt-claim",
            "gt-woke-noack" => "R3b-same-hold-and-hunt-the-ack-register-the-battery-is-the-oracle",
            "fw-acked-gt-dark" => "STOP-well-acks-but-ring-block-still-gated-report-before-another-write",
            "gt-live-already" => "R4-ggtt-claim(read-only)-holding-forcewake-from-here-on",
            "both-req-preheld" => "STOP-request-registers-held-by-another-owner-investigate-before-any-write",
            _ => "STOP-neither-published-forcewake-pair-decodes-on-gen7-ladder-decision-goes-to-Peter",
        }
    );
    serial_println!(":: gen7: r3 end ::");

    // Hand R4 the wake reading. The mapping is the verdict ladder above, condensed to the one
    // distinction R4 acts on: is the GT block reachable enough to attempt a single reversible
    // GGTT write, or must R4 stay read-only? `gt-woke` / `gt-woke-noack` / `gt-live-already`
    // are the three arms with positive evidence of a reachable GT; everything else — including
    // `fw-acked-gt-dark` (the well acked but the ring block stayed gated) — is `Dark`.
    match verdict {
        "gt-woke" => GtWake::Woke,
        "gt-woke-noack" => GtWake::WokeNoAck,
        "gt-live-already" => GtWake::LiveAlready,
        _ => GtWake::Dark,
    }
}

// ===================================================================================
// R4 — the GGTT claim. The read-only-then-reversible setup toward command submission.
// ===================================================================================
//
// ## What R4 is, in one paragraph
//
// The ring the render/blitter engine reads its commands from lives in the GGTT (draft G5:
// engine-visible addresses are GGTT offsets), so the first thing any command-submission path
// needs is a GGTT page it may safely own. R4 answers "can we claim a GGTT page, reversibly,
// on this part?" It is the rung the draft calls "R4 (drafted as R3) — can we claim GGTT pages
// safely?" (§5), sharpened by the GR26 brief into a rung that **reads R3's verdict and
// branches**, so it is correct whether or not the forcewake acquire opened the well.
//
// ## The two branches, and why each is decisive
//
//  * **R3 woke the GT (`GtWake::reachable()`).** R4 reads the RCS ring block (identifying the
//    four registers command submission will program — G3, IVB-PINNED), verifies a candidate
//    GGTT window is entirely unowned (every pre-image and both bracketing neighbours read
//    zero), and then performs ONE reversible PTE round-trip on the first slot: write a
//    well-formed PTE, read back the `0 → pte` transition, check the neighbours did not smear,
//    restore the whole touched neighbourhood to zero, and re-read to prove it. This is the
//    draft's read-only refusal law (§5, "refuse any range whose pre-images are not all zero")
//    plus the single write the GR26 brief asks for — a real dark→live→dark transition on a
//    page-table entry, under R3's exact discipline (entry image, reversible, restore-verify).
//  * **R3 did NOT wake the GT (`GtWake::Dark`).** This is the outcome Boot D's R2
//    `gt-still-dark` makes most likely, and R4 must not blindly write GGTT PTEs behind a
//    closed well. So it runs the IDENTICAL read-only census — the ring block and the candidate
//    window's pre-images — and reports `verdict=gated-on-wake` loudly, writing nothing. The
//    rung still produces a verdict (what the GGTT and ring registers read through the closed
//    well) and moves the GEN7-vs-Kepler question, rather than being a witness that silently
//    does nothing.
//
// ## Wall D, and the honesty of the transition witness
//
// A powered-down or gated GGTT window may read `0x00000000` for everything (draft §4.4). So
// the write path's success is NOT "we wrote a PTE" — it is `slot_held == pte`, a specific
// non-zero value only our write could have put there, read back from a slot whose pre-image we
// verified was zero. If the GGTT is gated even on a woken GT (the well is held for the render
// domain but the GGTT needs it too, or R3 released it — see below), `slot_held` stays zero,
// `landed=0`, and the rung prints `verdict=claim-write-void` — an honest reading, not a
// success. The neighbour-smear check is the tree's own (`igpu.rs` ~869-885), reused verbatim
// in intent: a store that bleeds into an adjacent PTE is a silent corruption, so "unchanged"
// is verified on both the held and the restored reads, never asserted.
//
// ## Forcewake is RELEASED here, deliberately, and the rung says so
//
// R3 releases both forcewake requests on every exit path (its reversibility contract), so R4
// runs with the well NOT held. That is the correct test: the GGTT PTE array (GTTMMADR, BAR0 +
// 2 MiB) is documented as memory-interface state, and Intel's own CHV note scopes the
// well-alive requirement to "access outside shadow register space" — GGTT PTEs are plausibly
// inside it. So the EXPECTED woke-branch result is `claim-roundtrip-ok` with the well released.
// If instead the round-trip is `claim-write-void`, that is a real finding — the GGTT needs the
// well held — and it reassigns the write into the wake's scope for R5, rather than being
// mistaken for a dead engine. Either way R4 does not re-hold forcewake: acquiring it is R3's
// job and re-implementing it here would duplicate the exact code the lane keeps in one place.
//
// ## Reversibility — three PTEs, all into a verified-zero window, all restored
//
// The only writes R4 ever makes are on the woke branch, into a window it first read as
// entirely zero, and they are undone before the rung returns: it writes the PTE to the target
// slot and (on the restore) zero to the target and both neighbours — every one of which was
// verified zero at entry, so writing zero to a neighbour is an identity, and writing the PTE
// then zero to the target is a clean undo. The final read of all three proves `restored`. No
// clock, no RC6/RPS policy, no voltage/frequency request, and nothing in a display block is
// written — the panel is the Kepler's regardless (`igpu.rs` ~778, Boot AS).

/// The candidate GGTT window R4 inspects and (on the woke branch) claims one slot of.
///
/// `CLAIM_FIRST` is a slot well inside the derived 524288-entry array and clear of the low
/// slots firmware tends to populate; `CLAIM_COUNT` is a ring page plus a scratch-surface
/// margin, matching the draft's "large enough for a ring (1 page) AND for a scratch surface
/// (say 64 pages)". The choice is only ever decisive because it is REFUSED unless every
/// pre-image in the window — and both bracketing neighbours — reads zero: an owned window is
/// named and skipped, never overwritten.
const CLAIM_FIRST: usize = 0x40000; // slot 262144 — mid-array; an R1 GGTT-census sample point
const CLAIM_COUNT: usize = 64; // 1 ring page + 63 scratch pages

/// GEN7 rung R4 — inspect the GGTT and, if R3 woke the GT, claim one page reversibly.
///
/// Called from `igpu::init` immediately after `forcewake` (R3) and still BEFORE
/// `bring_up_blt_ring`, taking R3's `GtWake` verdict so it branches on real evidence.
///
/// # Safety
/// Same contract as `recon` / `wake` / `forcewake`: `bar0` is a live MMIO mapping of at least
/// `bar0_size` bytes of the IGD's BAR0. R4 writes at most three GGTT PTEs, only when
/// `wake.reachable()`, only into a window every slot of which it first read as zero, and it
/// restores all three to zero and re-reads before returning. It writes no ring register and no
/// display register.
pub unsafe fn claim(bar0: usize, bar0_size: usize, bus: u8, slot: u8, func: u8, wake: GtWake) {
    serial_println!(
        ":: gen7: r4 begin rung=R4 wake={} reachable={} bdf={}:{}.{} bar0_size={} ladder=GEN7-3D ::",
        wake.name(),
        if wake.reachable() { 1 } else { 0 },
        bus,
        slot,
        func,
        bar0_size
    );

    // ---- Parachutes, identical discipline to R1/R2/R3 ---------------------------------
    if bar0 == 0 {
        serial_println!(":: gen7: r4 verdict=refused-no-bar0 — BAR0 unpublished; no read or write attempted ::");
        serial_println!(":: gen7: r4 next=STOP-bar0-unpublished-fix-igpu-init-mapping-first note=no-ggtt-touched ::");
        serial_println!(":: gen7: r4 end ::");
        return;
    }

    // `GTT_BASE = 0x200000` is [EXT-UNPINNED], carried from `igpu::regs::GTT_BASE` exactly as
    // R1 carries it; the wire says `base_pin=unpinned` for the same reason it does there.
    const GTT_BASE: usize = 0x200000; // igpu::regs::GTT_BASE — [EXT-UNPINNED]
    const SLOTS: usize = 524_288;
    // The highest byte R4 reads or writes is the neighbour AFTER the last claimed slot. Refuse
    // — before any read of the window and any write — if the window runs off the derived array
    // or off the enumerated BAR0, checked against the size we were GIVEN, not a literal.
    let win_last_slot = CLAIM_FIRST + CLAIM_COUNT; // inclusive: the "next" neighbour
    let win_end_off = GTT_BASE + (win_last_slot + 1) * 4;
    if win_last_slot >= SLOTS || bar0_size < win_end_off {
        serial_println!(
            ":: gen7: r4 verdict=refused-bar0-too-small have={} need={} last_slot={} slots={} — the derived GGTT window does not fit; no GGTT reading claimed ::",
            bar0_size,
            win_end_off,
            win_last_slot,
            SLOTS
        );
        serial_println!(":: gen7: r4 next=STOP-ggtt-window-out-of-range-refit-CLAIM_FIRST/COUNT note=no-ggtt-touched ::");
        serial_println!(":: gen7: r4 end ::");
        return;
    }

    // ---- Identify the ring buffer registers (G3, IVB-PINNED), read-only, BOTH branches ----
    // These are the four registers command submission will program (RING_START holds the GGTT
    // address of the ring; RING_CTL bit 0 enables it). R3 released forcewake, so on the woke
    // branch they may still read dark — the reading is reported, never assumed. `probe` reads
    // each twice and classifies, so a `varies=1` here would itself be proof of a running ring.
    let mut ring = Tally::default();
    probe(bar0, "rcs", "RING_START", g7regs::RCS_RING_START, "IVB-PINNED", &mut ring);
    probe(bar0, "rcs", "RING_CTL", g7regs::RCS_RING_CTL, "IVB-PINNED", &mut ring);
    probe(bar0, "rcs", "RING_HEAD", g7regs::RCS_RING_HEAD, "IVB-PINNED", &mut ring);
    probe(bar0, "rcs", "RING_TAIL", g7regs::RCS_RING_TAIL, "IVB-PINNED", &mut ring);
    serial_println!(
        ":: gen7: r4 ring verdict n={} structured={} zero={} allones={} varies={} note=ring-lives-in-GGTT-these-are-the-submission-registers ::",
        ring.n,
        ring.structured,
        ring.zero,
        ring.allones,
        ring.varies
    );

    // ---- Read-only census of the candidate window, BOTH branches --------------------------
    // Every pre-image is read. Only NON-zero slots are printed individually (the ones that
    // decide the refusal — an owned window must name which slots are owned); the common
    // all-zero window is summarised. The two bracketing neighbours are read too: a claim is
    // only safe if the slots on either side of the window are also unowned.
    let mut zeros = 0u32;
    let mut nonzero = 0u32;
    let mut first_nonzero_slot: i64 = -1;
    let mut first_nonzero_pte = 0u32;
    for k in 0..CLAIM_COUNT {
        let s = CLAIM_FIRST + k;
        let pte = rd(bar0, GTT_BASE + s * 4);
        if pte == 0 {
            zeros += 1;
        } else {
            nonzero += 1;
            if first_nonzero_slot < 0 {
                first_nonzero_slot = s as i64;
                first_nonzero_pte = pte;
            }
            serial_println!(
                ":: gen7: r4 ggtt owned slot={} off={:06X} pte={:08X} pfn={:05X} ::",
                s,
                GTT_BASE + s * 4,
                pte,
                pte >> 12
            );
        }
    }
    let prev_nb = rd(bar0, GTT_BASE + (CLAIM_FIRST - 1) * 4);
    let next_nb = rd(bar0, GTT_BASE + (CLAIM_FIRST + CLAIM_COUNT) * 4);
    let range_empty = nonzero == 0 && prev_nb == 0 && next_nb == 0;
    serial_println!(
        ":: gen7: r4 ggtt-claim first={} count={} zeros={} nonzero={} prev={:08X} next={:08X} first_nonzero_slot={} range_empty={} base={:06X} base_pin=unpinned ::",
        CLAIM_FIRST,
        CLAIM_COUNT,
        zeros,
        nonzero,
        prev_nb,
        next_nb,
        first_nonzero_slot,
        if range_empty { 1 } else { 0 },
        GTT_BASE
    );

    // ---- Branch on R3's verdict -----------------------------------------------------------
    if !wake.reachable() {
        // DARK branch — the read-only recon, and it writes NOTHING. This is the arm Boot D's
        // R2 `gt-still-dark` makes most likely. The verdict is loud on purpose: the rung read
        // the GGTT and the ring registers through the closed well and it is GATED on the wake,
        // which is a finding that moves the GEN7-vs-Kepler question, not a null.
        serial_println!(
            ":: gen7: r4 verdict=gated-on-wake wake={} ring_structured={}/{} range_empty={} writes=0 note=R3-did-not-wake-the-GT-no-GGTT-write-attempted-behind-a-closed-well ::",
            wake.name(),
            ring.structured,
            ring.n,
            if range_empty { 1 } else { 0 }
        );
        serial_println!(
            ":: gen7: r4 next=STOP-gated-on-forcewake-GEN7-vs-Kepler-decision-goes-to-Peter-with-R2-gt-still-dark-and-R3-fw-no-ack note=no-ggtt-touched ::"
        );
        serial_println!(":: gen7: r4 end ::");
        return;
    }

    // WOKE branch — R3 produced positive evidence the GT block is reachable. The window must
    // be verified unowned before a single write; an owned window is refused, never overwritten
    // (draft §5 refusal law).
    if !range_empty {
        serial_println!(
            ":: gen7: r4 verdict=range-owned-refused first_nonzero_slot={} first_nonzero_pte={:08X} prev={:08X} next={:08X} writes=0 note=something-owns-the-window-pick-another-never-overwrite-a-populated-PTE ::",
            first_nonzero_slot,
            first_nonzero_pte,
            prev_nb,
            next_nb
        );
        serial_println!(":: gen7: r4 next=STOP-candidate-window-owned-choose-another-CLAIM_FIRST note=no-ggtt-written ::");
        serial_println!(":: gen7: r4 end ::");
        return;
    }

    // ---- The reversible claim — one PTE round-trip, real page, restore-verified -----------
    // A real translated scratch page so the PTE is a genuine address (no fabricated constant),
    // exactly as `igpu::bring_up_blt_ring` builds the ring PTE.
    let layout = Layout::from_size_align(4096, 4096).unwrap();
    let page = alloc_zeroed(layout);
    if page.is_null() {
        serial_println!(":: gen7: r4 verdict=claim-alloc-failed writes=0 note=scratch-page-alloc-returned-null-nothing-written ::");
        serial_println!(":: gen7: r4 next=STOP-no-scratch-page note=no-ggtt-written ::");
        serial_println!(":: gen7: r4 end ::");
        return;
    }
    let Some(phys64) = crate::arch::memory::translate(page as u64) else {
        dealloc(page, layout);
        serial_println!(":: gen7: r4 verdict=claim-virt-unmapped va={:016X} writes=0 note=scratch-page-not-mapped-nothing-written ::", page as usize);
        serial_println!(":: gen7: r4 next=STOP-scratch-va-unmapped note=no-ggtt-written ::");
        serial_println!(":: gen7: r4 end ::");
        return;
    };
    let phys = phys64 as usize;
    // Gen7 GGTT PTEs carry extended physical bits 39:32 in PTE bits 7:4, which this claim does
    // not program (draft G6) — refuse, do not truncate, anything above 4 GiB. Same guard as the
    // tree's ring bring-up (`igpu.rs` ~841).
    if phys >= 0x1_0000_0000 {
        dealloc(page, layout);
        serial_println!(":: gen7: r4 verdict=claim-phys-above-4g phys={:016X} writes=0 note=extended-PTE-bits-not-programmed-nothing-written ::", phys);
        serial_println!(":: gen7: r4 next=STOP-scratch-phys-above-4g note=no-ggtt-written ::");
        serial_println!(":: gen7: r4 end ::");
        return;
    }
    let pte = (phys as u32) | 1; // valid bit — draft G6, the exact form the tree writes
    let target_off = GTT_BASE + CLAIM_FIRST * 4;
    let prev_off = target_off - 4;
    let next_off = target_off + 4;

    // Entry images. The whole window is verified zero, but re-read the three touched slots
    // here so the transition is measured against a fresh baseline, not a stale one.
    let slot_pre = rd(bar0, target_off);
    let prev_pre = rd(bar0, prev_off);
    let next_pre = rd(bar0, next_off);

    // Write the PTE, then read back the target and both neighbours under the hold.
    wr(bar0, target_off, pte);
    let slot_held = rd(bar0, target_off);
    let prev_held = rd(bar0, prev_off);
    let next_held = rd(bar0, next_off);
    // The transition witness: a specific non-zero value only our write could have put in a slot
    // whose pre-image was zero. A gated GGTT reads back zero and this is 0 — `landed=0`.
    let landed = slot_held == pte && slot_pre == 0;
    let smear_held = prev_held != prev_pre || next_held != next_pre;

    // Restore: write zero to the target (undo) and to both neighbours (identity — each was
    // verified zero at entry), then re-read all three to prove the neighbourhood is back.
    wr(bar0, prev_off, 0);
    wr(bar0, target_off, 0);
    wr(bar0, next_off, 0);
    let slot_post = rd(bar0, target_off);
    let prev_post = rd(bar0, prev_off);
    let next_post = rd(bar0, next_off);
    let restored = slot_post == 0 && prev_post == 0 && next_post == 0;
    let smear_post = prev_post != prev_pre || next_post != next_pre;
    let clean = restored && !smear_held && !smear_post;

    // The scratch frame returns to the allocator ONLY when the neighbourhood is proven back to
    // zero and nothing smeared. A GGTT PTE that still mapped this frame after a free would hand
    // the GT a DMA path into reused kernel heap — so if the reversal did not verify we LEAK the
    // page (a one-shot, boot-time 4 KiB cost) rather than surrender a possibly-mapped frame.
    // This is the rung's own refuse-on-any-doubt discipline applied to its single free.
    let freed = if clean {
        dealloc(page, layout);
        true
    } else {
        false
    };

    serial_println!(
        ":: gen7: r4 claim target_off={:06X} slot={} phys={:016X} pte={:08X} slot_pre={:08X} slot_held={:08X} slot_post={:08X} landed={} restored={} ::",
        target_off,
        CLAIM_FIRST,
        phys,
        pte,
        slot_pre,
        slot_held,
        slot_post,
        if landed { 1 } else { 0 },
        if restored { 1 } else { 0 }
    );
    serial_println!(
        ":: gen7: r4 claim-neighbours prev_off={:06X} prev_pre={:08X} prev_held={:08X} prev_post={:08X} next_off={:06X} next_pre={:08X} next_held={:08X} next_post={:08X} smear_held={} smear_post={} ::",
        prev_off,
        prev_pre,
        prev_held,
        prev_post,
        next_off,
        next_pre,
        next_held,
        next_post,
        if smear_held { 1 } else { 0 },
        if smear_post { 1 } else { 0 }
    );

    // Verdict — every arm named before the rung ran. A smear is convicted even though it was
    // cleaned up: a GGTT store that bleeds into a neighbour is a silent corruption nothing may
    // be built on. `claim-write-void` is the honest reading of a woken GT whose GGTT is still
    // gated with the well released — the finding the "forcewake released here" note names.
    let verdict = if smear_held || smear_post {
        "claim-smear"
    } else if !landed {
        "claim-write-void"
    } else if !restored {
        "claim-restore-dirty"
    } else {
        "claim-roundtrip-ok"
    };
    serial_println!(
        ":: gen7: r4 verdict={} wake={} landed={} restored={} smear={} writes=4 slots=3 freed={} reversible={} rung=R4 note=forcewake-released-by-R3-GGTT-claim-ran-with-well-not-held-page-freed-only-when-reversal-verified ::",
        verdict,
        wake.name(),
        if landed { 1 } else { 0 },
        if restored { 1 } else { 0 },
        if smear_held || smear_post { 1 } else { 0 },
        if freed { 1 } else { 0 },
        if clean { 1 } else { 0 }
    );
    serial_println!(
        ":: gen7: r4 next={} note=one-PTE-round-trip-restored-and-verified-no-display-register-touched ::",
        match verdict {
            "claim-roundtrip-ok" =>
                "R5-map-the-ring-page-and-submit-MI_STORE_DATA_IMM-into-a-GGTT-scratch-dword",
            "claim-write-void" =>
                "STOP-GGTT-gated-with-well-released-R5-must-hold-forcewake-around-the-claim",
            "claim-restore-dirty" =>
                "STOP-restore-did-not-verify-do-not-build-on-an-unproven-reversal",
            _ => "STOP-GGTT-write-smeared-a-neighbour-do-not-submit-a-ring-through-this-array",
        }
    );
    serial_println!(":: gen7: r4 end ::");
}
