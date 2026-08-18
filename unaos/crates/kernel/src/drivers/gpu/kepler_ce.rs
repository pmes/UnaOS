//! CE-LADDER — copy-engine reconnaissance on the GK107 (GT 650M).
//!
//! Design of record: `~/unaos-bench/scratch/gr24/CE-LADDER-draft.md` (scratch, GR24).
//! Standing metal facts: `docs/dev/OS/08_VIDEO/KEPLER-METAL-LOG.md`.
//! Ordering/parachute law inherited from `docs/dev/OS/08_VIDEO/falcon_microcode_spec.md` §5.
//!
//! # Why this module exists
//!
//! The compositor's present half is the prize, and it is now MEASURED, not assumed.
//! Boot A (`~/unaos-bench/capture/gr25-bootA/ttyUSB0.log`) [METAL]:
//!
//! ```text
//! [comp2] rollup passes=2150 pass_us=5214 … blit_us=5211 compose_us=1374 present_us=3832 …
//! ```
//!
//! `present_us` is ~73 % of the blit and ~12x the draft's 300 µs stop threshold. R0 is
//! ANSWERED and the ladder is GO.
//!
//! # Citation law (draft §0) — three classes, never mixed
//!
//! * `[METAL]`  — observed on THIS GK107, in a named capture.
//! * `[TREE]`   — stated by a repo document or by `kepler.rs` itself.
//! * `[EXT-UNPINNED]` — public-knowledge recollection, **not** verified against an opened
//!   document. Usable ONLY as a hypothesis this ladder tests on our own silicon.
//!
//! ⚠ **Clean room.** `docs/MANIFESTO/CLEAN_ROOM_POLICY.md` §2/§5 [TREE]: the GPL `nouveau`
//! driver source is not a source for anything here, ever. envytools rnndb/hwdocs
//! *documentation* would be Group-A-legal, but **no document was opened for this module**,
//! so every register number below that is not metal-proven carries `[EXT-UNPINNED]` at its
//! own definition and is treated as a hypothesis under test. Observing our own hardware is
//! the Group-A activity that produces documentation we may then implement against — which
//! is why every rung's real deliverable is a witness line, not code.
//!
//! # What is implemented here, and what is deliberately NOT
//!
//! | Rung | Status |
//! | --- | --- |
//! | R0 present/compose split | ANSWERED [METAL Boot A] — lives in `video/wm.rs`, not here |
//! | R1 PTOP device-info sweep | **here**, read-only |
//! | R2 CE PRI window probe | **here**, read-only |
//! | R2b first CE WRITE (arm) | **here** — one authored-magic scratch write, self-gated on R2's FALCON-REST verdict, entry-captured + restored + read back |
//! | R3 runlist scan (wide) | **here**, read-only |
//! | R4 BAR1-identity | ANSWERED [METAL Boot A] — `kepler.rs`, see [`R4_RESULT`] |
//! | R5 firmware instance-block walk | **here**, one metal-proven window write, restored |
//! | R7+ genuine byte copy / CE channel | NOT here. Those are CONSUMERS of R1/R2/R3/R5/R2b answers |
//!
//! R2b is the campaign's FIRST copy-engine write, scoped down from "submit a bounded copy"
//! because a genuine byte copy needs the CE datapath method encoding — a fact only a CE
//! channel (draft R6, gated on an unflown runlist id + an unaudited RAMFC layout) or the CE
//! falcon datapath map (UNKNOWN) can give. R2b instead proves the necessary precondition of
//! any copy — that a confirmed-live CE PRI window latches an authored write — reversibly.
//! R7 and a real channel are gated by the draft §5 on facts this boot does not yet have.
//! Building a consumer against an unknown is how the PGRAPH channel spent eleven boots; this
//! module takes the largest reversible step and stops.
//!
//! # Parachute discipline
//!
//! * **Two device writes, both reversible and restore-verified, everything else read-only:**
//!   (1) the PRAMIN window base (R5), metal-proven (R4, Boot A), saved + restored + **read
//!   back** at every use; (2) R2b's SINGLE authored-magic write to a CE falcon's
//!   `CC_SCRATCH[0]` (`+0x800`), which fires ONLY behind a same-boot FALCON-REST verdict, is
//!   entry-captured, restored, and read back, and touches no datapath/enable/clock/fifo
//!   register. A restore written but never read is a success echo that cannot fail; a
//!   failure is counted, surfaced on the witness line, and VOIDs that rung's verdict.
//! * **`0x504` is never read at any base.** The first access to a falcon's `+0x504` wedges
//!   every later read in that unit for the boot (spec §5.4). A `const _` assertion below
//!   proves the offset table cannot contain it.
//! * **Every rung is bracketed** by a control read of `NV_PMC_BOOT_0` (spec §5.4): a rung
//!   whose bracket moved is reported as such, so "the space is dead" is never confused with
//!   "the space is empty".
//! * **Honest VOID.** A poison / floating / refused read is named POISON / ABSENT / VOID and
//!   draws no conclusion. `classify_fecs_word` is the single verdict vocabulary.
//! * **No unbounded loops.** There are no loops here that are not `for i in 0..N`.

use super::kepler::{classify_fecs_word, mmio_read, mmio_write, regs};

// ===========================================================================
// Register numbers — each with its citation class AT ITS DEFINITION
// ===========================================================================

/// PTOP device-info table base. **[EXT-UNPINNED]** (draft C6). The single
/// highest-value probe in the study, and entirely unproven on this part: R1
/// exists to pin or kill it.
const PTOP_DEVICE_INFO: usize = 0x0002_2700;
/// Entries swept. 64 dwords is the draft's stated stimulus (R1).
const PTOP_ENTRIES: usize = 64;

/// Copy-engine PRI bases. **[EXT-UNPINNED]** (draft C1). R2 settles them from the chip.
const CE_BASES: [usize; 3] = [0x0010_4000, 0x0010_5000, 0x0010_6000];

/// Falcon register block offsets, **[TREE]** — `falcon_microcode_spec.md` §2 proves this
/// shape on FECS (`0x409000`) and GPCCS (`0x41A000`) on this exact chip. R2 asks whether a
/// CE base answers with the same shape (draft C2).
///
/// ⛔ `0x504` is ABSENT BY CONSTRUCTION and the `const _` below proves it.
const FALCON_PROBE_OFFSETS: [(usize, &str); 7] = [
    (0x000, "r000"),   // first touch: a nonexistent PRI answers BADFxxxx here
    (0x040, "mb0"),
    (0x044, "mb1"),
    (0x100, "cpuctl"),
    (0x10C, "dmactl"),
    (0x800, "cc0"),
    (0x804, "cc1"),
];

// Two compile-time proofs, not comments.
//
//   1. The poison offset can never enter the probe table. Spec §5.4 cost this campaign
//      three sittings; a comment saying "don't add 0x504" would not have stopped any of
//      them.
//   2. Every CE base is 0x1000-aligned. The probe offsets are added to a base with `+`, so
//      a base carrying low bits would silently SHIFT the whole falcon register window and
//      every value read through it would be a plausible-looking lie about the wrong
//      register — the exact failure mode the campaign's "merely-plausible value" rule
//      exists to catch (draft §4.4).
const _: () = {
    let mut i = 0;
    while i < FALCON_PROBE_OFFSETS.len() {
        assert!(FALCON_PROBE_OFFSETS[i].0 != 0x504, "CE probe may never touch +0x504");
        i += 1;
    }
    let mut b = 0;
    while b < CE_BASES.len() {
        assert!(CE_BASES[b] & 0xFFF == 0, "CE PRI base must be 0x1000-aligned");
        b += 1;
    }
};

/// FECS's documented rest values on this chip [METAL, Boot A `fal-base … cpuctl=00000010`,
/// and spec §2]. Two independent registers agreeing on the documented rest state is the
/// strongest evidence available short of an authored magic (draft R2).
const FALCON_REST_CPUCTL: u32 = 0x0000_0010;
/// `DMACTL.REQUIRE_CTX` set at rest [TREE, spec §2].
const FALCON_REST_DMACTL: u32 = 0x0000_0001;

/// Named falcon-block offsets R2b (the first CE write) touches or re-verifies, **[TREE]**
/// (spec §2, proven on FECS/GPCCS on this chip). Named consts, not literals at the write
/// site, so the safety assertions below can prove what R2b may and may not reach.
///
/// * `CPUCTL`/`DMACTL` — read only, to RE-VERIFY the FALCON-REST gate at write time.
/// * `CC0` = `CC_SCRATCH[0]` — the ONE register R2b writes. Spec §3.2 proved falcon IO for
///   `0x800/0x804` on this silicon with an authored magic (`A55E7A55`), so it is the single
///   safest first-write target: plain scratch, no datapath, no side effect.
const FALCON_CPUCTL_OFF: usize = 0x100;
const FALCON_DMACTL_OFF: usize = 0x10C;
const FALCON_CC0_OFF: usize = 0x800;

/// The R2b authored magics. **High-entropy, and NEITHER is `0x00000002`** — draft §4.4's
/// standing mandate after the Boot AS `0x118` finding: "the next port experiment must assert
/// a high-entropy magic, not `CHAN_VALID`'s bit pattern." The ALT exists so that on the
/// (astronomically unlikely) boot where the scratch already holds the primary magic, the
/// write is still a REAL change and `landed` cannot be a success echo of an unchanged word.
const CE_ARM_MAGIC: u32 = 0xCE5A_A502;
const CE_ARM_MAGIC_ALT: u32 = 0x5AA5_CE10;

// Compile-time proof that R2b's write target is the scratch register and NOT one of the two
// forbidden offsets: `0x504` (first touch wedges the unit, spec §5.4) and `0x118`
// (`DMATRFCMD` — the Boot AS accidental-DMA finding the draft flagged FOR SAFETY REVIEW,
// §4.4). A comment would not have stopped the three sittings §5.4 cost; an assertion does.
const _: () = {
    assert!(FALCON_CC0_OFF != 0x504, "R2b may never write +0x504 (poison, spec §5.4)");
    assert!(FALCON_CC0_OFF != 0x118, "R2b may never write +0x118 DMATRFCMD (draft §4.4)");
    assert!(FALCON_CC0_OFF == 0x800, "R2b writes CC_SCRATCH[0] and nothing else");
};

/// Host runlist controls. `0x2270/0x2274` is the pair this driver submits to [TREE,
/// `kepler.rs`]. `0x2280 + i*8` is the array shape the draft's C7 proposes
/// **[EXT-UNPINNED]** — and Boot A already showed `0x2280/0x2284` aliasing our own
/// submission exactly, so the array is real at i=0 at least [METAL].
const RL_SUBMIT_BASE: usize = 0x0000_2270;
const RL_ARRAY_BASE: usize = 0x0000_2280;
const RL_ARRAY_N: usize = 8;

/// PRAMIN window base register, in units of 64 KiB, and the aperture it opens.
///
/// **[METAL — Boot A]**, and this is the one thing in this module that is not a
/// hypothesis. `bar1-identity` wrote `win = scratch_off >> 16` here, read the authored
/// `CEA50BA5` back at `0x700000 + (scratch_off & 0xFFFF)`, and restored the pre-value:
///
/// ```text
/// :: kepler: bar1-identity scratch_off=02015000 magic=CEA50BA5 bar1_rb=CEA50BA5
///            win_pre=00003FF0 win_want=00000201 win_rb=00000201 pramin_read=CEA50BA5
///            win_restored=Y ::
/// ```
///
/// That single line pins draft C9's window register, its 64 KiB granularity, and the
/// aperture address, all at once — R5 stands on it rather than on recollection.
const PBUS_BAR0_WINDOW: usize = 0x0000_1700;
const PRAMIN_BASE: usize = 0x0070_0000;
const PRAMIN_WIN_BYTES: usize = 0x1_0000;

/// Candidate BAR1 / BAR3 instance-block pointer registers. **[EXT-UNPINNED]** (draft C9).
/// R5 reads both and lets the dump say which, if either, names a real structure.
const INST_PTR_REGS: [(usize, &str); 2] = [(0x0000_1704, "bar1?"), (0x0000_1714, "bar3?")];

/// The R4 answer, carried here so no later rung re-derives it or re-runs the probe.
///
/// **[METAL — Boot A]: `IDENTITY`.** A BAR1 offset IS a physical VRAM address on this part.
/// Draft §4.3 — the wall that could have invalidated every FIFO pointer this driver has
/// ever written — is CLOSED as a false alarm, and R5 may treat a VRAM offset and a physical
/// page number as the same number.
pub const R4_RESULT: &str = "IDENTITY (Boot A) — BAR1 offset == physical VRAM address";

// ===========================================================================
// Shared instrument helpers
// ===========================================================================

/// A control read of a known-good register at both ends of a rung (spec §5.4).
///
/// `NV_PMC_BOOT_0` is the chip ID: constant, cheap, and outside every window this module
/// touches. If it moves across a rung, the rung's reads are not interpretable — and saying
/// so is the whole point. A sweep that returns all zeros with a HELD bracket means "the
/// space is empty"; the same sweep with a MOVED bracket means "the bus is sick".
struct Bracket {
    pre: u32,
}

impl Bracket {
    fn open(bar0: usize) -> Self {
        Self { pre: unsafe { mmio_read(bar0, regs::NV_PMC_BOOT_0) } }
    }

    /// Returns `(pre, post, verdict)`.
    fn close(&self, bar0: usize) -> (u32, u32, &'static str) {
        let post = unsafe { mmio_read(bar0, regs::NV_PMC_BOOT_0) };
        let verdict = if post == self.pre {
            "held"
        } else {
            "MOVED — the control register changed across this rung; NOTHING it read is interpretable"
        };
        (self.pre, post, verdict)
    }
}

/// The outcome of one windowed PRAMIN read.
///
/// Two independent failures live here and they must not be collapsed: the window can refuse
/// the value we point it at (the read is VOID), and the window can refuse to go BACK
/// (every later PRAMIN user this boot is suspect, whatever this read returned). A single
/// `Option` could only carry the first.
struct PraminRead {
    /// `None` when the window register did not take the value written — a VOID about the
    /// WINDOW, never an answer about the target page.
    val: Option<u32>,
    /// `false` when the window did not read back as its pre-value after the restore.
    restored: bool,
}

/// Point the PRAMIN window at `phys`, read one dword, then restore the window **and read
/// the restore back**.
///
/// The restore readback is the difference between "we wrote the old value" and "the old
/// value is there". This module's whole claim to being reversible rests on the latter, and
/// it costs one `mmio_read`.
fn pramin_read(bar0: usize, phys: usize) -> PraminRead {
    let win_pre = unsafe { mmio_read(bar0, PBUS_BAR0_WINDOW) };
    let want = (phys / PRAMIN_WIN_BYTES) as u32;
    unsafe { mmio_write(bar0, PBUS_BAR0_WINDOW, want) };
    let win_rb = unsafe { mmio_read(bar0, PBUS_BAR0_WINDOW) };
    let val = if win_rb == want {
        Some(unsafe { mmio_read(bar0, PRAMIN_BASE + (phys % PRAMIN_WIN_BYTES)) })
    } else {
        None
    };
    // Restore FIRST, report after — an unrestored window silently corrupts every later user.
    unsafe { mmio_write(bar0, PBUS_BAR0_WINDOW, win_pre) };
    let win_post = unsafe { mmio_read(bar0, PBUS_BAR0_WINDOW) };
    PraminRead { val, restored: win_post == win_pre }
}

// ===========================================================================
// R1 — does this chip have copy engines, and where?
// ===========================================================================

/// Sweep the PTOP device-info table, raw first.
///
/// **Stimulus:** 64 BAR0 reads at `PTOP_DEVICE_INFO + i*4`. No writes.
///
/// **The raw dword is the datum.** C6's field layout is [EXT-UNPINNED] and a wrong decode
/// that overwrote the raw value would destroy the only thing this rung actually produces,
/// so the decode is a strictly secondary, explicitly-labelled line and the raw appears in a
/// compact row dump that no interpretation can touch.
///
/// **Readings this separates (draft R1):**
/// * PASS — structured non-zero dwords terminating in zeros, at least one whose
///   hypothesised PRI base is `0x104000`-class.
/// * REFUTED-CLEANLY — all `0xBADFxxxx`: the table is not here on this part; C6 dies and R1
///   must be re-derived from a Group-A document before another boot is spent.
/// * AMBIGUOUS — all zeros with a HELD bracket: unpopulated or unclocked, and the bracket
///   is what tells that from a dead bus.
fn r1_ptop(bar0: usize) -> &'static str {
    let br = Bracket::open(bar0);

    let mut raw = [0u32; PTOP_ENTRIES];
    for (i, slot) in raw.iter_mut().enumerate() {
        *slot = unsafe { mmio_read(bar0, PTOP_DEVICE_INFO + i * 4) };
    }

    // Raw dump, 8 per line: the FTDI boot ring is 64 KiB drop-oldest (see kepler.rs
    // MIRROR_HDR_DENSE) and 64 single-entry lines would cost the display leg its capture.
    for row in 0..(PTOP_ENTRIES / 8) {
        let b = row * 8;
        serial_println!(
            ":: kepler: ce-ptop row i={:02}..{:02} [{:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X}] ::",
            b, b + 7,
            raw[b], raw[b + 1], raw[b + 2], raw[b + 3],
            raw[b + 4], raw[b + 5], raw[b + 6], raw[b + 7]
        );
    }

    // Per-entry detail lines are CAPPED. The row dump above already carries every raw
    // dword, so these lines are commentary, not data — and 64 of them at ~250 bytes would
    // be ~16 KiB of the 64 KiB drop-oldest FTDI boot ring, enough to evict the display and
    // ucode legs from the capture (the same budget that silenced MIRROR_HDR_DENSE). The
    // elision is COUNTED and printed, so a reader can never mistake a capped list for a
    // short one.
    const ENTRY_LINE_CAP: usize = 16;

    let mut nonzero = 0usize;
    let mut poison = 0usize;
    let mut ce_candidates = 0usize;
    let mut entry_lines = 0usize;
    let mut elided = 0usize;
    for (i, &v) in raw.iter().enumerate() {
        let cls = classify_fecs_word(v);
        if cls == "POISON" {
            poison += 1;
            continue;
        }
        if v == 0 {
            continue;
        }
        nonzero += 1;

        // ⚠ HYPOTHESIS, draft C6, [EXT-UNPINNED]. NOTHING may be built on these fields.
        // Under C6 an entry carries a PRI base in units of 0x1000 together with a runlist
        // id, an engine id and a reset bit. We do not know the bit positions, so we print
        // the ONE extraction the rung needs — a PRI-base candidate — and flag a hit only
        // when it lands in the 0x100000..0x110000 neighbourhood C1 names. A miss here is
        // not evidence against copy engines; it is evidence against C6's layout.
        let pri_candidate = ((v as usize) & 0x0000_0FFF) << 12;
        let is_ce_neighbourhood = (0x0010_0000..0x0011_0000).contains(&pri_candidate);
        if is_ce_neighbourhood {
            ce_candidates += 1;
        }
        if entry_lines < ENTRY_LINE_CAP {
            entry_lines += 1;
            serial_println!(
                ":: kepler: ce-ptop entry i={:02} raw={:08X} {} | HYPOTHESIS(C6,EXT-UNPINNED) pri_candidate={:06X} ce_neighbourhood={} — the RAW is the datum; this decode is an untested layout guess ::",
                i, v, cls, pri_candidate, if is_ce_neighbourhood { "Y" } else { "n" }
            );
        } else {
            elided += 1;
        }
    }
    if elided > 0 {
        serial_println!(
            ":: kepler: ce-ptop entry-elided shown={} elided={} cap={} — ring budget only; every elided dword is in the `ce-ptop row` dump above and the counters below cover ALL {} entries ::",
            entry_lines, elided, ENTRY_LINE_CAP, PTOP_ENTRIES
        );
    }

    // ⛔ THE BRACKET CLOSES BEFORE THE VERDICT IS SELECTED, and `ctl` is the FIRST arm.
    // A verdict chosen first and a bracket printed afterwards is a confident sentence about
    // a bus that may have been dead the whole time — spec §5.4 requires the control read to
    // GATE the reading, not merely to accompany it.
    let (pre, post, ctl) = br.close(bar0);
    let held = ctl == "held";

    let verdict = if !held {
        "VOID — the NV_PMC_BOOT_0 control bracket MOVED across this sweep. Every dword above is uninterpretable and NO reading is drawn: this says nothing about whether a device-info table exists at 0x022700"
    } else if poison == PTOP_ENTRIES {
        "REFUTED-CLEANLY — every entry is a nonexistent-PRI fault response. The device-info table is NOT at 0x022700 on this part; C6 is dead and R1 must be re-derived from a Group-A document before another boot is spent"
    } else if nonzero == 0 && poison == 0 {
        "AMBIGUOUS — the bracket HELD and the space answers, but every entry is zero: unpopulated, or the unit is unclocked. Those two are not separated here; do NOT write a guessed PMC_ENABLE bit to find out"
    } else if ce_candidates > 0 {
        "PASS — structured entries present, at least one hypothesised PRI base in the 0x104000 neighbourhood C1 names. C6 and C1 corroborate each other; the decoded base is a CANDIDATE for R2, not yet a fact"
    } else {
        "PARTIAL — the table is populated but no entry decodes into the CE PRI neighbourhood. Either C6's layout is wrong (most likely) or this part carries no copy engine. R2 is the independent check and it does not depend on this decode"
    };

    serial_println!(
        ":: kepler: ce-ptop verdict entries={} nonzero={} poison={} ce_candidates={} shown={} elided={} ctl_pre={:08X} ctl_post={:08X} ctl={} ::",
        PTOP_ENTRIES, nonzero, poison, ce_candidates, entry_lines, elided, pre, post, ctl
    );
    serial_println!(":: kepler: ce-ptop VERDICT {} ::", verdict);

    // The rollup token carries the voiding too — a `CE-LADDER end` reading `r1_ptop=refuted`
    // off a dead bus would be the same lie one line further out.
    if !held {
        "void"
    } else if poison == PTOP_ENTRIES {
        "refuted"
    } else if nonzero == 0 {
        "ambiguous"
    } else if ce_candidates > 0 {
        "pass"
    } else {
        "partial"
    }
}

// ===========================================================================
// R2 — are the CE PRI windows alive, and are they falcons?
// ===========================================================================

/// Probe each candidate CE PRI base with the falcon register shape spec §2 already proves
/// on this chip.
///
/// **Placed AFTER R1** by the draft's ordering: R1's decoded base is the number this rung
/// would rather have had, and printing R1 first means a reader can see whether the two
/// agree. R2 does **not** consume R1's decode, deliberately — an independent check that
/// shares no assumption with the thing it checks is worth more than a chained one.
///
/// **Readings (draft R2):**
/// * FALCON-REST — `cpuctl == 0x10` **and** `dmactl == 0x01`. C2 confirmed at this base:
///   the unit exists and is a falcon, which opens draft §4.2 path 2 (a CE falcon as a bare
///   DMA microcontroller, no PFIFO, no channel, no FECS wall).
/// * PRESENT-NOT-FALCON — the WHOLE window read without a fault and carries structured
///   values that are not the falcon rest pattern. The unit exists and path 2 closes for
///   that base. C2 half-refuted.
/// * ABSENT — the first read is a fault response. That base does not exist.
/// * ABSENT-PARTIAL — a later offset faulted. Spec §5.4's retroactive lesson is that after
///   a fault only the FIRST datum is trustworthy, so the probe STOPS at the fault, prints
///   the remainder as `--`, and claims nothing about falcon-ness. Each base also gets its
///   own bracket for the same reason.
/// * AMBIGUOUS — all zeros. Candidate explanation is a clear `NV_PMC_ENABLE` bit, and the
///   bit number is supposed to come from R1's reset field. **No blind PMC_ENABLE write is
///   made here** — pull 28's ban on unproven writes is in force.
/// Returns `(rollup_token, first_falcon_base)`. The second element is the target R2b's write
/// is self-gated on: `Some(base)` only when that base read the exact FALCON-REST pair this
/// boot, `None` otherwise. R2b writes to no base this rung did not just confirm live.
fn r2_ce_probe(bar0: usize) -> (&'static str, Option<usize>) {
    let pmc_en = unsafe { mmio_read(bar0, regs::NV_PMC_ENABLE) };
    serial_println!(
        ":: kepler: ce-probe begin bases={} pmc_enable={:08X} — recorded read-only so a later boot can correlate an ABSENT base with an enable bit. NO PMC_ENABLE WRITE IS MADE: the bit number must come from R1's reset field, not from a guess (pull 28) ::",
        CE_BASES.len(), pmc_en
    );

    let mut falcon = 0usize;
    let mut present = 0usize;
    let mut absent = 0usize;
    let mut ambiguous = 0usize;
    let mut falcon_base: Option<usize> = None;

    for &base in CE_BASES.iter() {
        // Its OWN bracket, per base — a fault at base N must not be allowed to explain
        // away the reads at base N+1.
        let br = Bracket::open(bar0);

        // ⛔ STOP AT THE FIRST FAULT. Spec §5.4's retroactive lesson is that after a fault
        // only the FIRST datum is trustworthy — so continuing to read this base and then
        // printing six more values beside the fault would be this rung violating the very
        // rule it cites. Everything past the fault is printed as `--`, which is a different
        // claim from `00000000`.
        let mut vals = [0u32; FALCON_PROBE_OFFSETS.len()];
        let mut read_n = 0usize;
        for (k, &(off, _)) in FALCON_PROBE_OFFSETS.iter().enumerate() {
            let v = unsafe { mmio_read(bar0, base + off) };
            vals[k] = v;
            read_n = k + 1;
            if classify_fecs_word(v) == "POISON" {
                break;
            }
        }
        let (pre, post, ctl) = br.close(bar0);

        // Index by name so a reordering of the table cannot silently rename a value.
        // Returns `None` for an offset the loop never reached.
        let get = |name: &str| -> Option<u32> {
            let mut v = None;
            for (k, &(_, n)) in FALCON_PROBE_OFFSETS.iter().enumerate() {
                if n == name && k < read_n {
                    v = Some(vals[k]);
                }
            }
            v
        };
        let show = |name: &str| -> alloc::string::String {
            match get(name) {
                Some(v) => alloc::format!("{:08X}", v),
                None => alloc::string::String::from("--"),
            }
        };
        let r000 = get("r000").unwrap_or(0);
        let cpuctl = get("cpuctl");
        let dmactl = get("dmactl");

        // A fault ANYWHERE in the window, not just at r000, is a poisoned base: a BADFxxxx
        // at cpuctl with a structured r000 is not "a unit that is not a falcon", it is a
        // unit whose window is partly nonexistent, and calling that PRESENT would be the
        // merely-plausible-value error one level up.
        let faulted = (0..read_n).any(|k| classify_fecs_word(vals[k]) == "POISON");

        serial_println!(
            ":: kepler: ce-probe base={:06X} read_n={}/{} r000={} mb0={} mb1={} cpuctl={} dmactl={} cc0={} cc1={} cls_r000={} ctl_pre={:08X} ctl_post={:08X} ctl={} ::",
            base, read_n, FALCON_PROBE_OFFSETS.len(),
            show("r000"), show("mb0"), show("mb1"), show("cpuctl"), show("dmactl"),
            show("cc0"), show("cc1"),
            classify_fecs_word(r000), pre, post, ctl
        );

        let verdict = if ctl != "held" {
            ambiguous += 1;
            "VOID — the control bracket moved across this base; these reads say nothing"
        } else if classify_fecs_word(r000) == "POISON" {
            absent += 1;
            "ABSENT — the FIRST read of this base returned the nonexistent-PRI fault response and the probe stopped there. This base does not exist on this part"
        } else if faulted {
            absent += 1;
            "ABSENT-PARTIAL — the base's first dword is structured but a LATER offset faulted, and the probe stopped at that offset. A window that is partly nonexistent is not a unit; nothing about falcon-ness is claimed, and the read_n field above names where it stopped"
        } else if cpuctl == Some(FALCON_REST_CPUCTL) && dmactl == Some(FALCON_REST_DMACTL) {
            falcon += 1;
            if falcon_base.is_none() {
                falcon_base = Some(base);
            }
            "FALCON-REST — cpuctl=0x10 AND dmactl=0x01, the exact documented rest pair FECS shows on this chip. C2 CONFIRMED at this base: the unit exists and is a falcon, and draft §4.2 path 2 (CE falcon as a bare DMA microcontroller, no PFIFO) opens. R2b will take this base as its authored-write target"
        } else if vals[..read_n].iter().any(|&v| v != 0 && v != 0xFFFFFFFF) {
            present += 1;
            "PRESENT-NOT-FALCON — the whole window read without a fault and carries structured values that are not the falcon rest pattern. The unit exists and is not a falcon; C2 is half-refuted and path 2 closes for this base"
        } else {
            ambiguous += 1;
            "AMBIGUOUS — the space answers (bracket held, no fault) but every probed register is zero: unpopulated, or unclocked. The candidate explanation is an NV_PMC_ENABLE bit, and that number comes from R1, never from a guessed write"
        };
        serial_println!(":: kepler: ce-probe base={:06X} VERDICT {} ::", base, verdict);
    }

    serial_println!(
        ":: kepler: ce-probe verdict bases={} falcon={} present={} absent={} ambiguous={} arm_target={} ::",
        CE_BASES.len(), falcon, present, absent, ambiguous,
        match falcon_base { Some(b) => b, None => 0 }
    );

    let token = if falcon > 0 { "falcon" } else if present > 0 { "present" } else if absent == CE_BASES.len() { "absent" } else { "ambiguous" };
    (token, falcon_base)
}

// ===========================================================================
// R2b — the FIRST copy-engine WRITE (the largest reversible step this boot)
// ===========================================================================

/// Write a high-entropy authored magic to a confirmed-live CE falcon's scratch, read it
/// back, and restore the captured entry value — the campaign's first CE write.
///
/// # Why this, and not a byte copy
///
/// The brief's R2 is "the first actual copy-engine operation." A genuine VRAM→VRAM copy
/// (draft R7) needs the CE datapath method encoding, which needs ONE of two things this boot
/// does not have:
/// * a PFIFO channel on a CE runlist (draft R6) — gated on a CE runlist id (R1/R3, UNFLOWN)
///   and an audited instance-block layout (R5, UNFLOWN; the constants in `kepler.rs` are
///   still UNAUDITED), and it drags in `nvidia-kepler-fifo`; or
/// * the CE falcon's internal datapath register map — UNKNOWN (draft §4.2 path 2 blocker a),
///   and its one documented DMA command register `+0x118` is the very offset Boot AS may have
///   hit by accident and the draft flagged FOR SAFETY REVIEW (§4.4).
///
/// Both are gated behind facts only the read-only recon above can produce. Per the brief's
/// escape hatch — "scope R2 to the largest reversible step and document what the next rung
/// needs" — R2b is that step: the operation that is BOTH a real register state transition
/// AND the mandatory precondition of any copy submission — prove the CE PRI window LATCHES
/// an authored write. It also discharges draft §4.4's standing mandate to re-assert the
/// falcon port rule at a CE base with a high-entropy magic rather than `CHAN_VALID`'s `0x2`.
///
/// # Safety envelope — every clause a STOP-tripwire the draft names
///
/// * **Self-gated.** Fires only when `r2_ce_probe` reported FALCON-REST at `falcon_base` THIS
///   boot, and re-verifies `cpuctl==0x10 && dmactl==0x01` with a fresh read immediately
///   before the write. `None` ⇒ nothing is written and the boot stays exactly as read-only
///   as the recon-only ladder. This is the expected first-flight arm, because the CE bases
///   are entirely unproven until this same boot's `ce-probe` speaks.
/// * **One register.** Touches `+0x800` (`CC_SCRATCH[0]`) ONLY — never `+0x504` (poison),
///   never `+0x118` (`DMATRFCMD`), never any enable/clock/reset/fifo register. The `const _`
///   above proves the target at compile time. No datapath command is issued.
/// * **Entry-image capture + restore-verify.** The pre-value is saved, restored, and the
///   restore is READ BACK. A restore written but never read is a success echo that cannot
///   fail; a failed restore is `NOT-RESTORED`, never a quiet pass.
/// * **Bracketed** by `NV_PMC_BOOT_0`; a moved bracket VOIDs the verdict.
///
/// # Readings (the witness reads a REAL state transition; it cannot pass on a dead base)
///
/// * `SKIPPED` — no FALCON-REST base this boot. Nothing written.
/// * `REST-MOVED` — the rest pair did not re-verify at write time. Nothing written.
/// * `ARMED` — the magic was written, read back EQUAL (the transition `entry → magic`), and
///   the entry value restored and verified. The CE latches authored state; a copy has a
///   proven-live, writable target. **This is the pass, and it required a real change.**
/// * `REJECTED` — a falcon at rest that did NOT hold the magic. C2's writability half is
///   refuted here; the window answers reads but does not latch a write.
/// * `NOT-RESTORED` / `VOID` — the restore did not read back, or the bracket moved.
fn r2b_ce_arm(bar0: usize, falcon_base: Option<usize>) -> &'static str {
    let base = match falcon_base {
        Some(b) => b,
        None => {
            serial_println!(
                ":: kepler: ce-r2 SKIPPED — r2_ce_probe named no FALCON-REST base this boot, so there is no proven-live target and NOTHING is written. The ladder stays as read-only as the recon-only pass; R2b arms only behind a same-boot FALCON-REST verdict (draft §5: no consumer against an unknown) ::"
            );
            return "skipped";
        }
    };

    let br = Bracket::open(bar0);

    // Re-verify the FALCON-REST gate with a FRESH read at write time. The gate is not
    // inherited from a value read microseconds ago in another rung; it is re-established
    // here, immediately before the only write this module makes to a CE base.
    let cpuctl = unsafe { mmio_read(bar0, base + FALCON_CPUCTL_OFF) };
    let dmactl = unsafe { mmio_read(bar0, base + FALCON_DMACTL_OFF) };
    if cpuctl != FALCON_REST_CPUCTL || dmactl != FALCON_REST_DMACTL {
        let (pre, post, ctl) = br.close(bar0);
        serial_println!(
            ":: kepler: ce-r2 base={:06X} REST-MOVED cpuctl={:08X} dmactl={:08X} — the FALCON-REST gate did not re-verify at write time (rest pair is cpuctl=00000010 dmactl=00000001); NOTHING is written ctl_pre={:08X} ctl_post={:08X} ctl={} ::",
            base, cpuctl, dmactl, pre, post, ctl
        );
        return "rest-moved";
    }

    // Entry-image capture of the ONE register R2b touches.
    let entry = unsafe { mmio_read(bar0, base + FALCON_CC0_OFF) };
    // Pick a magic that is guaranteed DIFFERENT from the entry value, so `landed` can never
    // be a success echo of an already-equal word (spec §4.2's "merely-plausible value" trap).
    let magic = if entry == CE_ARM_MAGIC { CE_ARM_MAGIC_ALT } else { CE_ARM_MAGIC };

    // The write, then the readback that IS the state transition.
    unsafe { mmio_write(bar0, base + FALCON_CC0_OFF, magic) };
    let observed = unsafe { mmio_read(bar0, base + FALCON_CC0_OFF) };

    // Restore the captured entry value FIRST, then READ THE RESTORE BACK. Restore before
    // report — an unrestored scratch silently perturbs the unit for every later reader.
    unsafe { mmio_write(bar0, base + FALCON_CC0_OFF, entry) };
    let restored_rb = unsafe { mmio_read(bar0, base + FALCON_CC0_OFF) };

    let (pre, post, ctl) = br.close(bar0);
    let held = ctl == "held";
    let landed = observed == magic;
    let restored = restored_rb == entry;

    serial_println!(
        ":: kepler: ce-r2 base={:06X} reg={:03X} entry={:08X} magic={:08X} observed={:08X} restored_rb={:08X} landed={} restored={} ctl_pre={:08X} ctl_post={:08X} ctl={} ::",
        base, FALCON_CC0_OFF, entry, magic, observed, restored_rb,
        if landed { "Y" } else { "N" }, if restored { "Y" } else { "N" }, pre, post, ctl
    );

    // ⛔ THE BRACKET GATES THE VERDICT, and a broken restore outranks a landed write: leaving
    // the unit perturbed is a worse outcome than a copy target that did not latch.
    let verdict = if !held {
        "VOID — the NV_PMC_BOOT_0 bracket MOVED across the write; the readback is uninterpretable and NO state transition is claimed"
    } else if !restored {
        "NOT-RESTORED — the authored magic was written but the entry value did NOT read back after the restore. CC_SCRATCH[0] is left perturbed at this base; the reversibility claim is BROKEN and the next rung must treat this base as dirty"
    } else if landed {
        "ARMED — the high-entropy magic was written to the CE falcon's CC_SCRATCH[0], read back EQUAL, and the captured entry value restored and verified. The copy engine LATCHES authored state: draft §4.4's port rule is re-established at a CE base with a proper magic (not 0x2), and a copy submission now has a proven-live, writable target. This is the campaign's FIRST copy-engine write, and it required a real entry->magic transition"
    } else {
        "REJECTED — the base re-verified as a falcon at rest but did NOT hold the authored magic (observed != magic). C2's writability half is refuted at this base: the window answers reads yet does not latch a write, so it is not plain scratch and the copy path needs a different entry than CC_SCRATCH"
    };
    serial_println!(":: kepler: ce-r2 base={:06X} VERDICT {} ::", base, verdict);

    if !held {
        "void"
    } else if !restored {
        "not-restored"
    } else if landed {
        "armed"
    } else {
        "rejected"
    }
}

// ===========================================================================
// R3 — where do runlists live, and what does bit 20 actually mean?
// ===========================================================================

/// Wide sweep of the host runlist array, plus a re-read of the pair this driver submits to.
///
/// **This runs PRE-SUBMIT, and that is the design.** The ladder is called before the
/// `nvidia-kepler-fifo` leg builds and submits its channel, so **anything populated here is
/// the FIRMWARE's, not ours** — which is exactly the discrimination the draft's "second
/// purpose" asks for, obtained for free instead of by inference. A populated slot at this
/// point is direct evidence of a non-PGRAPH runlist, and its index is the runlist id R6
/// would need.
///
/// Boot A's narrower in-FIFO sweep [METAL] read `i=2 base_off=2280 base=00002013
/// len=00100003 … OCCUPIED` — but that was AFTER our own submit, so the only occupied slot
/// it could see was ours. It also read `i=1 base_off=2278 base=BAD0011F POISON`, which
/// refutes the array-stride assumption at `0x2270`: element 1 of THAT array does not exist.
/// The sweep did not find "no CE runlist"; it found "not there". This rung looks where C7
/// says to look, and eight slots wide.
///
/// **The three readings of bit 20 it separates (draft R3):**
/// * **pending** — set on our slot and clear on the others. Then Boot A's
///   `playlist_rd_len=00100003` says the scheduler has NEVER consumed our runlist, which is
///   a competing root cause for the whole `err=2` wall and far cheaper to fix.
/// * **id field** — slot `i` reads `i << 20` regardless of content. Then we have been
///   submitting to the wrong runlist id all along.
/// * **unconditional** — every populated slot has it. Bit 20 stops being evidence,
///   permanently.
fn r3_runlist_scan(bar0: usize) -> &'static str {
    let br = Bracket::open(bar0);

    let submit_base = unsafe { mmio_read(bar0, RL_SUBMIT_BASE) };
    let submit_len = unsafe { mmio_read(bar0, RL_SUBMIT_BASE + 4) };

    let mut populated = 0u32;
    let mut bit20_set = 0u32;
    let mut engfield_is_index = true;
    let mut any_populated = false;

    for i in 0..RL_ARRAY_N {
        let base_off = RL_ARRAY_BASE + i * 8;
        let len_off = base_off + 4;
        let rl_base = unsafe { mmio_read(bar0, base_off) };
        let rl_len = unsafe { mmio_read(bar0, len_off) };
        let cls = classify_fecs_word(rl_base);
        let bit20 = (rl_len >> 20) & 1;
        let engf = (rl_len >> 20) & 0xF;
        let entries = rl_len & 0xFFF;

        let state = if cls == "POISON" {
            "POISON — this slot does not exist; the array is shorter than the sweep"
        } else if rl_base != 0 {
            any_populated = true;
            populated |= 1 << i;
            if bit20 != 0 {
                bit20_set |= 1 << i;
            }
            if engf != i as u32 {
                engfield_is_index = false;
            }
            "OCCUPIED — pre-submit, so this runlist is the FIRMWARE's, not ours"
        } else {
            "empty"
        };
        serial_println!(
            ":: kepler: ce-rlscan i={} base@{:04X}={:08X} len@{:04X}={:08X} bit20={} engfield={:X} entries={} {} ::",
            i, base_off, rl_base, len_off, rl_len, bit20, engf, entries, state
        );
    }

    // ⛔ THE BRACKET CLOSES BEFORE THE READING IS SELECTED, and `ctl` is the FIRST arm. Of
    // all four rungs this is the one where a MOVED bus is most dangerous: "we have been
    // submitting to the wrong runlist id all along" is a sentence that would re-open the
    // whole fence arc, and a dead bus reads all-zeros, which is exactly the shape that
    // makes engfield==i true for free.
    let (pre, post, ctl) = br.close(bar0);
    let held = ctl == "held";

    // ID-FIELD needs a real sample. `engfield_is_index` starts TRUE and is only ever
    // falsified, so one populated slot at i=0 with engfield 0 would "confirm" it without
    // any evidence at all — 0 == 0 is not a pattern. Require at least two populated slots
    // AND at least one away from index 0, so the claim rests on a coincidence that would
    // have to happen twice.
    let n_pop = populated.count_ones();
    let has_nonzero_index = (populated & !1u32) != 0;
    let id_field_supported = engfield_is_index && n_pop >= 2 && has_nonzero_index;

    let reading = if !held {
        "VOID — the NV_PMC_BOOT_0 control bracket MOVED across this sweep. A dead bus reads all-zeros, which is the exact shape that makes 'engfield == index' true for free, so NO reading of bit 20 is drawn and nothing is claimed about runlist ids"
    } else if !any_populated {
        "NO-DATA — nothing is populated pre-submit. Bit 20 is untestable this pass, and the CE payoff (a firmware-left runlist for another engine) is absent: either the firmware left none, or the array is not here"
    } else if id_field_supported {
        "ID-FIELD — every populated slot reads engfield == its own index, across at least two slots and at least one away from index 0, so bits 23:20 are a runlist/engine ID and Boot A's 00100003 reads 'runlist 1, 3 entries'. We have been submitting to the wrong runlist id all along, and that is a competing root cause for the entire wall"
    } else if engfield_is_index {
        "INCONCLUSIVE (n=1) — engfield equals the slot index on every populated slot, but the sample cannot carry the claim: fewer than two slots are populated, or the only one is index 0 where 0==0 holds by arithmetic rather than by evidence. ID-FIELD is NOT asserted"
    } else if bit20_set == populated {
        "UNCONDITIONAL — bit 20 is set on every populated slot regardless of content. It stops being evidence permanently, and gpu_spec.md §2.4.1 can finally close on it"
    } else {
        "DISCRIMINATING — bit 20 is set on some populated slots and clear on others, so it carries information rather than being a constant. ⚠ WHICH information is NOT settled here: this rung runs PRE-SUBMIT, so none of these slots is ours and the ours-vs-firmware comparison that would name bit 20 a COMMIT-PENDING bit cannot be made from this pass. Pair this line with the in-fifo runlist-scan from the same boot to make that call"
    };

    serial_println!(
        ":: kepler: ce-rlscan submit-pair base@{:04X}={:08X} len@{:04X}={:08X} {} ::",
        RL_SUBMIT_BASE, submit_base, RL_SUBMIT_BASE + 4, submit_len,
        if submit_base == 0 { "ZERO — expected pre-submit; this rung runs before the fifo leg writes it" } else { "NON-ZERO pre-submit — somebody wrote this pair before us" }
    );
    serial_println!(
        ":: kepler: ce-rlscan verdict populated={:02X} bit20_set={:02X} foreign={} n_pop={} idx_gt0={} phase=pre-submit ctl_pre={:08X} ctl_post={:08X} ctl={} ::",
        populated, bit20_set, n_pop, n_pop, if has_nonzero_index { "Y" } else { "n" }, pre, post, ctl
    );
    serial_println!(":: kepler: ce-rlscan VERDICT bit20={} ::", reading);

    if !held {
        "void"
    } else if !any_populated {
        "no-data"
    } else if id_field_supported {
        "id-field"
    } else if engfield_is_index {
        "inconclusive-n1"
    } else if bit20_set == populated {
        "unconditional"
    } else {
        "discriminating"
    }
}

// ===========================================================================
// R5 — read the firmware's own working channel (the legal RAMFC route)
// ===========================================================================

/// Walk the firmware's instance block through PRAMIN.
///
/// **Why this is the clean-room route.** `kepler.rs`'s instance-block constants are
/// UNAUDITED and are forbidden to be called validated until a layout comes from a Group-A
/// source (`CLEAN_ROOM_POLICY.md` §5, and the standing 2026-08-08 provenance-ledger entry)
/// [TREE]. There is a Group-A source sitting in the machine: the firmware left a live
/// channel and page directory in VRAM. Observing our own hardware is white-box reverse
/// engineering — the policy's §2 Group-A activity — and it produces documentation we may
/// then implement against. That is what this rung is for.
///
/// **What it buys:** (a) a real instance-block layout with in-house provenance, which
/// discharges the standing clean-room debt; (b) the answer to draft C8 — if the firmware's
/// channel carries a page directory, CE addresses are virtual and R6 grows a whole era;
/// (c) a template for R6.
///
/// **Its one write** is the PRAMIN window base, and that register is no longer a
/// hypothesis: see [`R4_RESULT`] and `PBUS_BAR0_WINDOW`. It is saved and restored at every
/// single use inside `pramin_read`, and **the restore is read back** — "we wrote the old
/// value" and "the old value is there" are different claims, and only the second one makes
/// this rung reversible.
///
/// **Failure mode, stated so it is not evaded:** if the dump is all zeros or all poison,
/// C9's pointer registers are wrong and NOTHING ELSE IS CLAIMED. We do **not** fall back to
/// "it probably looks like the one in `kepler.rs`" — quoting our own unaudited code back as
/// its own authority is the exact circularity the withdrawn 2026-08-08 audit committed.
fn r5_inst_walk(bar0: usize, vram_size: usize) -> &'static str {
    let outer = Bracket::open(bar0);
    let win_pre = unsafe { mmio_read(bar0, PBUS_BAR0_WINDOW) };
    serial_println!(
        ":: kepler: ce-inst begin win_pre={:08X} vram={}MB r4={} — the window register is METAL-PROVEN (Boot A bar1-identity); every use saves it, restores it, and READS THE RESTORE BACK ::",
        win_pre, vram_size >> 20, R4_RESULT
    );

    let mut walked = 0usize;
    let mut with_content = 0usize;
    let mut voided = 0usize;
    let mut restore_fail_total = 0usize;

    for &(reg, label) in INST_PTR_REGS.iter() {
        // ⛔ ONE BRACKET PER REGISTER, the R2 idiom. A single bracket spanning the whole
        // loop cannot gate a per-register verdict: the first register's STRUCTURE line
        // would already be printed by the time the shared bracket closed, and a bus that
        // died halfway would leave the earlier verdict standing and confident.
        let br = Bracket::open(bar0);
        let mut restore_fail = 0usize;

        let val = unsafe { mmio_read(bar0, reg) };
        let cls = classify_fecs_word(val);

        // ⚠ HYPOTHESIS, draft C9, [EXT-UNPINNED]: the pointer is a 4 KiB page number in the
        // low bits with mode/target bits above. R4 proved a VRAM offset IS a physical
        // address here, so `page << 12` is a VRAM offset we may bound-check directly.
        let phys = ((val as usize) & 0x0FFF_FFFF) << 12;
        let in_range = phys != 0 && phys + 0x1000 <= vram_size;

        serial_println!(
            ":: kepler: ce-inst reg={:06X} ({}) val={:08X} {} | HYPOTHESIS(C9,EXT-UNPINNED) phys={:08X} in_vram={} ::",
            reg, label, val, cls, phys, if in_range { "Y" } else { "n" }
        );

        // The bracket is consulted BEFORE any verdict for this register, exactly as R2 does.
        {
            let (p0, p1, c0) = br.close(bar0);
            if c0 != "held" {
                voided += 1;
                serial_println!(
                    ":: kepler: ce-inst reg={:06X} VERDICT VOID — the NV_PMC_BOOT_0 control bracket MOVED across this register's read (ctl_pre={:08X} ctl_post={:08X}). The pointer value above is uninterpretable and the walk is NOT attempted ::",
                    reg, p0, p1
                );
                continue;
            }
        }

        if cls == "POISON" || cls == "ABSENT" || val == 0 {
            voided += 1;
            serial_println!(
                ":: kepler: ce-inst reg={:06X} VERDICT VOID — the pointer register itself reads {}; C9 named the wrong register and NOTHING about the firmware's channel is claimed ::",
                reg, cls
            );
            continue;
        }
        if !in_range {
            serial_println!(
                ":: kepler: ce-inst reg={:06X} VERDICT REFUSED — the hypothesised page {:08X} falls outside this part's {} MB of VRAM, so the decode is wrong and the walk is NOT attempted. A dump from an out-of-range window would be noise wearing a structure's clothes ::",
                reg, phys, vram_size >> 20
            );
            continue;
        }

        walked += 1;

        // 32 dwords of the instance block, 8 per line — raw, uninterpreted.
        let mut nonzero = 0usize;
        let mut refused = false;
        let mut words = [0u32; 32];
        for (k, w) in words.iter_mut().enumerate() {
            let r = pramin_read(bar0, phys + k * 4);
            if !r.restored {
                restore_fail += 1;
            }
            match r.val {
                Some(v) => {
                    *w = v;
                    if v != 0 {
                        nonzero += 1;
                    }
                }
                None => {
                    refused = true;
                    break;
                }
            }
        }
        if refused {
            voided += 1;
            restore_fail_total += restore_fail;
            serial_println!(
                ":: kepler: ce-inst reg={:06X} VERDICT VOID — the PRAMIN window register refused a value mid-dump. That contradicts Boot A's bar1-identity result and is a finding about the WINDOW, not about the firmware's channel ::",
                reg
            );
            continue;
        }
        for row in 0..4 {
            let b = row * 8;
            serial_println!(
                ":: kepler: ce-inst dump reg={:06X} off={:03X} [{:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X}] ::",
                reg, b * 4,
                words[b], words[b + 1], words[b + 2], words[b + 3],
                words[b + 4], words[b + 5], words[b + 6], words[b + 7]
            );
        }

        // Second checkpoint against the SAME `pre`: the dump above is 32 windowed reads and
        // a bus that died during them must not leave a STRUCTURE verdict standing.
        let (d0, d1, dctl) = br.close(bar0);
        if dctl != "held" {
            voided += 1;
            restore_fail_total += restore_fail;
            serial_println!(
                ":: kepler: ce-inst reg={:06X} VERDICT VOID — the control bracket MOVED across the dump (ctl_pre={:08X} ctl_post={:08X}); the 32 dwords above are uninterpretable and no layout is derived ::",
                reg, d0, d1
            );
            continue;
        }
        if restore_fail > 0 {
            voided += 1;
            restore_fail_total += restore_fail;
            serial_println!(
                ":: kepler: ce-inst reg={:06X} VERDICT VOID — the PRAMIN window failed to read back as its pre-value after {} of the dump's restores. This rung's reversibility claim is BROKEN for this register and every later PRAMIN user this boot is suspect; no layout is derived ::",
                reg, restore_fail
            );
            continue;
        }

        if nonzero == 0 {
            serial_println!(
                ":: kepler: ce-inst reg={:06X} VERDICT EMPTY — the window took every value, every restore read back, and every dword read zero. Either C9 named the wrong register, or that page is not an instance block. No layout is derived and the clean-room debt stands ::",
                reg
            );
            continue;
        }
        with_content += 1;
        serial_println!(
            ":: kepler: ce-inst reg={:06X} VERDICT STRUCTURE nonzero={}/32 restores_ok=32/32 ctl=held — a populated page at the hypothesised pointer. This dump is the in-house Group-A observation the RAMFC layout may be derived FROM; it is not itself a layout, and no offset in kepler.rs may be called validated on the strength of this line alone ::",
            reg, nonzero
        );

        // Page-directory candidate. ⚠ HYPOTHESIS on top of a hypothesis (draft C8): the
        // gf100-class instance block is recalled to carry a PD pointer near +0x200
        // [EXT-UNPINNED]. Printed as a SEPARATE, doubly-labelled line so that a wrong
        // guess here cannot contaminate the raw dump above, which stands on its own.
        // (The dump above stops at +0x7C, so the PD probe reads its own dwords.)
        let pd_a = pramin_read(bar0, phys + 0x200);
        let pd_b = pramin_read(bar0, phys + 0x204);
        if !pd_a.restored || !pd_b.restored {
            restore_fail += 1;
        }
        match (pd_a.val, pd_b.val) {
            (Some(a), Some(b)) => {
                let pd_phys = (((b as usize) & 0xFF) << 32) | ((a as usize) & 0xFFFF_F000);
                let pd_in_range = pd_phys != 0 && pd_phys + 0x20 <= vram_size;
                serial_println!(
                    ":: kepler: ce-inst pd-probe reg={:06X} +200={:08X} +204={:08X} | HYPOTHESIS(C8+C9, EXT-UNPINNED, layered) pd_phys={:08X} in_vram={} — if this names a real page directory then CE addresses are VIRTUAL and R6 needs page tables ::",
                    reg, a, b, pd_phys, if pd_in_range { "Y" } else { "n" }
                );
                if pd_in_range {
                    let mut pde = [0u32; 8];
                    let mut ok = true;
                    for (k, e) in pde.iter_mut().enumerate() {
                        let r = pramin_read(bar0, pd_phys + k * 4);
                        if !r.restored {
                            restore_fail += 1;
                        }
                        match r.val {
                            Some(v) => *e = v,
                            None => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if ok {
                        serial_println!(
                            ":: kepler: ce-inst pde reg={:06X} [{:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X}] — RAW. Whether these are page-directory entries at all is exactly what is UNKNOWN ::",
                            reg, pde[0], pde[1], pde[2], pde[3], pde[4], pde[5], pde[6], pde[7]
                        );
                    }
                }
            }
            _ => serial_println!(
                ":: kepler: ce-inst pd-probe reg={:06X} VOID — window refused during the PD read ::",
                reg
            ),
        }
        restore_fail_total += restore_fail;
    }

    let win_post = unsafe { mmio_read(bar0, PBUS_BAR0_WINDOW) };
    let (pre, post, ctl) = outer.close(bar0);
    let win_ok = win_post == win_pre;
    serial_println!(
        ":: kepler: ce-inst verdict regs={} walked={} with_content={} voided={} restore_fails={} win_pre={:08X} win_post={:08X} win_restored={} ctl_pre={:08X} ctl_post={:08X} ctl={} ::",
        INST_PTR_REGS.len(), walked, with_content, voided, restore_fail_total, win_pre, win_post,
        if win_ok { "Y" } else { "N — THE WINDOW WAS LEFT MOVED; every later PRAMIN user this boot is suspect" },
        pre, post, ctl
    );

    // The rollup token carries every voiding condition: a moved outer bracket, an unrestored
    // window, or any per-read restore failure all outrank a STRUCTURE result.
    if ctl != "held" || !win_ok || restore_fail_total > 0 {
        "void"
    } else if with_content > 0 {
        "structure"
    } else if walked > 0 {
        "empty"
    } else {
        "void"
    }
}

// ===========================================================================
// Entry point
// ===========================================================================

/// Run the read-only rungs of the CE ladder.
///
/// # Placement contract
///
/// This is called from `kepler::init` **before** the `nvidia-kepler-fifo` leg, and that
/// position is load-bearing in three separate ways:
///
/// 1. **Above every FECS access.** The FIFO leg's POKE ucode deliberately poisons the unit
///    and the terminal `0x409504` write closes the boot; the first access to that offset
///    wedges every later read in the unit (spec §5.4). Everything here is upstream of both.
/// 2. **Pre-submit, which makes R3 sharp.** Nothing in the runlist array is ours yet, so a
///    populated slot is unambiguously the firmware's — the CE payoff, by construction
///    rather than by inference.
/// 3. **Independent of the FIFO knob.** `UNAOS_KEPLER_CE` alone arms this; a boot may run
///    the reconnaissance without building a channel at all.
///
/// # QEMU says nothing
///
/// There is no Kepler in QEMU. A green `./arroyo check` proves the lattice and the types
/// and NOTHING about any rung. Do not cite emulation for a line in this file.
pub fn ladder(bar0: usize, vram_size: usize) {
    serial_println!(
        ":: kepler: CE-LADDER begin knob=UNAOS_KEPLER_CE rungs=R1,R2,R3,R5,R2b — READ-ONLY except (a) the PRAMIN window base, metal-proven (Boot A), restored+read-back at every use, and (b) R2b's SINGLE authored-magic write to a CE scratch, which fires ONLY behind a same-boot FALCON-REST verdict and is entry-captured, restored, and read back. R0=ANSWERED(present_us~73% of blit, Boot A) R4={} ::",
        R4_RESULT
    );

    let r1 = r1_ptop(bar0);
    let (r2, r2_falcon_base) = r2_ce_probe(bar0);
    let r3 = r3_runlist_scan(bar0);
    let r5 = r5_inst_walk(bar0, vram_size);

    // R2b — the first CE WRITE — goes LAST, after every read-only rung. Two reasons, both
    // §5.4 ordering law: (1) a boot's reconnaissance is complete and banked before the
    // campaign's first write, so a write-induced fault cannot contaminate any read above it;
    // (2) it is self-gated on r2_ce_probe's FALCON-REST base, captured above and consumed
    // here, so it never writes to a base this boot did not just confirm live.
    let r2b = r2b_ce_arm(bar0, r2_falcon_base);

    // A rollup that always prints. A probe whose absence cannot be distinguished from a
    // quiet pass is an instrument that cannot fire, and this repo has convicted three of
    // those. If the ladder ran, this line exists.
    serial_println!(
        ":: kepler: CE-LADDER end r1_ptop={} r2_ce_probe={} r3_rlscan={} r5_inst={} r2b_arm={} — next gate: a genuine VRAM->VRAM copy (draft R7) is WITHHELD until r2b reports ARMED (a writable CE target), r1/r3 name a CE runlist id, and r5 yields an audited instance-block layout OR the CE falcon datapath map is derived ::",
        r1, r2, r3, r5, r2b
    );
}
