//! KFBIND — derive the GK107 PBDMA register base from PTOP, then read the channel's
//! fetch pointers at the DERIVED base instead of the legacy guess.
//!
//! Register of record: [`SHUTOUT-REGISTER.md`] §2, rung **KF27 `kfbind`**. Queue row:
//! `docs/dev/OS/rmbp-queue.md` §GPU LADDERS, `KFBIND`. Knob `UNAOS_KEPLER_KFBIND=1`
//! (feature `nvidia-kepler-kfbind`, implies `nvidia-kepler` + `nvidia-kepler-fifo`).
//!
//! [`SHUTOUT-REGISTER.md`]: ../../../../../../docs/dev/OS/08_VIDEO/SHUTOUT-REGISTER.md
//!
//! # The wall this rung is aimed at, and why it is the base and not the bind
//!
//! K-GPU-3's wall since July is one line: **`gp_get=0` — the PBDMA never fetches entry
//! 0.** Sittings #4 and #5 each closed with the same NEXT item and neither has ever been
//! done (`KEPLER-METAL-LOG.md`):
//!
//! > s#5: "derive the correct GK107 PBDMA register base + the unit start/clock so `40108`
//! > reads real status."
//! > s#4: "which PBDMA unit is bound to our channel's runlist + its start/enable/clock
//! > sequence."
//!
//! Everything this campaign believes about the PBDMAs is read through ONE unproven
//! number. `kepler.rs` addresses them at `0x40000 + i * 0x2000` — a guess inherited from
//! sitting #3, where the previous guess (`0x6c0`) had just returned `0xBAD0011F` and was
//! abandoned for this one. At that base:
//!
//! * `bad-read pbdma 40108` returned **POISON at s#4 and ZERO at s#5** — the sitting's own
//!   note says "base likely right, but unit reads 0 = never started". A register that
//!   answers poison on one boot and zero on the next is equally well described as *not
//!   being there*, and nothing has ever discriminated the two.
//! * `DISCRIMINATOR pbdma{0,1,2} ch=00000000 (CHID=0 ACTIVE=0)` — quoted as a verdict in
//!   the register since s#6, unchanged through s#43. **A zero read at an unproven base is
//!   not the statement "the channel is not scheduled"; it is the statement "this address
//!   returned zero".** Those are different sentences and the ladder has been reading the
//!   second as the first for ten sittings.
//!
//! So the rung the register ranks first for K-GPU-3 is not another bind. It is the
//! derivation every bind's observable is read through. KF11→KF14 already paid for this
//! lesson four times over — *PMC bit 12 clear*, *the wrong base*, *the wrong base again*,
//! *DMACTL REQUIRE_CTX set* — and §2's own summary says it plainly: not one of those four
//! recorded failures was a property of the silicon.
//!
//! # ⭐ The tree-sourced finding this rung exists to test
//!
//! `kepler.rs` (the beacon-plant safety argument, ~:1785) carries this citation, and it is
//! the strongest one available in-tree:
//!
//! > envytools `docs/hw/fifo/dma-pusher.rst`, "Channel control area", enumerates every
//! > usable address in the area — DMA_PUT 0x40, DMA_GET 0x44, REF 0x48, DMA_PUT_HIGH 0x4C,
//! > 0x50 (GF100+), DMA_CGET 0x54, DMA_MGET 0x58/0x5C, DMA_GET_HIGH 0x60, **IB_GET 0x88,
//! > IB_PUT 0x8C** — and none of them lies below 0x40.
//!
//! The same comment records, two lines on, where this driver's deleted `gp_get`/`gp_put`
//! witness actually looked:
//!
//! > this driver's own historic GP_GET/GP_PUT witness (added 1c9e2570, removed 51b98bab at
//! > pull 15) read **0x8C/0x90**.
//!
//! Put the two side by side. `0x8C` is **IB_PUT** — the pointer the HOST writes — and
//! `0x90` is not in the enumerated list at all. **`IB_GET` (0x88), the one register that
//! says whether the PBDMA fetched anything, is not read by the deleted witness and is not
//! read anywhere in this tree today.** The campaign's founding observable, `gp_get=0`, may
//! never have been a reading of GP_GET.
//!
//! That is a discrepancy between two claims in the SAME in-tree comment, so it needs no
//! external document to state and none to test — which is exactly why it is this rung's
//! falsifier rather than a guessed PBDMA enable bit.
//!
//! # What this rung does, in order, and what it refuses to do
//!
//! 1. **PTOP device-info sweep** (`kfbind_pre`), bracketed. Raw dwords first; the decode is
//!    a strictly secondary, labelled line. Derives PBDMA PRI base CANDIDATES.
//! 2. **Pre-submit read, DERIVED base beside LEGACY base**, so the two are scored against
//!    each other in one capture instead of across two sittings. Plus the four channel
//!    control-area pointers, `IB_GET` among them for the first time.
//! 3. **No write.** See [`SKIPPED_WRITES`]. The bind/enable sequence this rung is named for
//!    needs the PBDMA `CHANNEL` enable bit encoding, and no source in this tree or opened
//!    for this worktree names it. Under `falcon_microcode_spec.md` §0.1 an `UNPINNED` claim
//!    "may be probed, never written", so the write is printed as SKIPPED with its reason
//!    and the rung ships `writes=0`. A first flight that derives the base is the input the
//!    write needs; inventing the write first is how the PGRAPH channel spent eleven boots.
//! 4. **Post-submit re-read** (`kfbind_post`) of exactly the same addresses, scored
//!    `FETCHED` / `STILL-DARK under <conditions>` (R19: named conditions, never
//!    "ruled out").
//!
//! # Parachute discipline
//!
//! * **Zero device writes.** Not "writes that are restored" — none. `restored=` therefore
//!   reports `n/a writes=0`, which is the honest form of the ladder's restore bound: a
//!   restore line on a rung that wrote nothing is a success echo that cannot fail.
//! * **Every rung bracketed** by `NV_PMC_BOOT_0` (spec §5.4), and the bracket GATES the
//!   verdict rather than accompanying it.
//! * **No probe outside [`PROBE_WINDOW`]**, proven by a `const _` below. A PTOP-derived
//!   candidate is an attacker-grade input — it is whatever the chip's table happens to
//!   hold — so it is range-checked before it is ever added to `bar0`. The window excludes
//!   PGRAPH/FECS (`0x400000..0x420000`) outright: `0x409504`'s first touch wedges the unit
//!   for the boot (spec §5.4), and a decode slip must not be able to reach it.
//! * **`ZERO` and `POISON` are printed, never interpreted.** `classify_fecs_word` is the
//!   single verdict vocabulary, shared with `kepler_ce.rs`.

use super::kepler::{classify_fecs_word, mmio_read, regs};

// ===========================================================================
// Register numbers — each with its citation class AT ITS DEFINITION
// ===========================================================================
//
// Tag vocabulary is `docs/dev/OS/08_VIDEO/falcon_microcode_spec.md` §0.1, verbatim:
//   (sitting id) observed on this bench · DERIVED follows from a proven rule ·
//   EXT external doc for a register this bench HAS ALSO observed ·
//   UNPINNED external or inferred with NO corroborating observation — probe only, never
//   the sole basis for a write.
//
// ⚠ NO rnndb FILE WAS OPENED FOR THIS MODULE. There is no envytools checkout in this
// worktree (`find ~ -iname '*rnndb*'` -> nothing). Where a line below names an rnndb file
// or field it is naming THE SOURCE THE NUMBER IS CLAIMED TO COME FROM, from recollection,
// which §0.1 classifies as UNPINNED and not as EXT — EXT requires this bench to have
// observed the register too. Every such number is a hypothesis this rung tests on our own
// silicon, and none of them is written to.

/// PTOP device-info table base. **[TREE + UNPINNED]** — `kepler_ce.rs::PTOP_DEVICE_INFO`
/// carries the same number under the same tag (draft C6), so the two rungs' sweeps are
/// comparable in one capture; the rnndb spelling recollected for it is the `PTOP`
/// device-info array (`NV_PTOP_DEVICE_INFO(i)`, stride 4). Never flown: CE-R1 is
/// `never-run` as of s#43, so this rung must not assume the table is there.
const PTOP_DEVICE_INFO: usize = 0x0002_2700;
/// Entries swept. Matches `kepler_ce.rs::PTOP_ENTRIES` so the two dumps line up.
const PTOP_ENTRIES: usize = 64;

/// The PRI-base extraction applied to a PTOP entry. **[TREE]** — deliberately the SAME
/// expression `kepler_ce.rs` R1 uses (`((v & 0xFFF) << 12)`), not a second guess. If the
/// two rungs ever disagree about a dword it is a code defect, not a hardware finding.
const fn ptop_pri_candidate(v: u32) -> usize {
    ((v as usize) & 0x0000_0FFF) << 12
}

/// The legacy PBDMA base this driver has always used: `0x40000 + i * 0x2000`.
/// **[TREE]** — `kepler.rs`'s KF9 `ctrladdr` audit and the `DISCRIMINATOR` loop both
/// address the units this way. Its provenance is a guess made at s#3 when the previous
/// guess returned poison; **this rung's entire job is to stop treating it as settled.**
const PBDMA_LEGACY_BASE: usize = 0x0004_0000;
const PBDMA_LEGACY_STRIDE: usize = 0x0000_2000;
/// `pbdma-count 3` [METAL s#5/s#6] — the population `NV_PMC_SUBFIFO_ENABLE` (0x204)
/// reports. Carried as the legacy sweep's width so the new lines pair 1:1 with every
/// historic `DISCRIMINATOR pbdma{0,1,2}` line in the capture archive.
const PBDMA_LEGACY_N: usize = 3;

/// Per-unit offsets inside a PBDMA PRI window, and what each one's status in this tree is.
///
/// * `+0x108` — the historic `bad-read pbdma 40108` address. **[TREE]**, and its two
///   readings disagree: POISON at s#4, ZERO at s#5, at the same address on the same part.
/// * `+0x120` — the `DISCRIMINATOR` channel register, decoded `CHID = [11:0]`,
///   `ACTIVE = bit 13`. **[TREE]** (`kepler.rs`), read as a verdict since s#6.
/// * `+0x008` / `+0x00C` — `CTRL_ADDR` low/high, the KF9 audit's subject. **[TREE]**;
///   every unit read `pre=00000000 hi=00000000` at s#13, which at an unproven base is a
///   READ-ZERO and not a state.
///
/// Read-only, all four, at BOTH bases.
const PBDMA_PROBE_OFFSETS: [(usize, &str); 4] = [
    (0x008, "ctrl_addr_lo"),
    (0x00C, "ctrl_addr_hi"),
    (0x108, "status"),
    (0x120, "chan"),
];

/// The channel control area, in USERD, reached through BAR1. **[TREE]** and this is the
/// best-cited table in the module: `kepler.rs`'s beacon-plant argument quotes envytools
/// `docs/hw/fifo/dma-pusher.rst` "Channel control area" for exactly these offsets.
///
/// `IB_GET` is first because it is the point of the rung: it is the pointer the PBDMA
/// advances when it fetches, and no capture in this campaign has ever contained it.
/// `0x090` is carried ONLY because the deleted witness read it — it is NOT in the
/// enumerated list, so it is printed as an unnamed word and no meaning is claimed for it.
const USERD_PROBE_OFFSETS: [(usize, &str); 5] = [
    (0x088, "ib_get"),          // the falsifier
    (0x08C, "ib_put"),          // what the deleted witness called gp_get
    (0x090, "x090_unnamed"),    // what the deleted witness called gp_put; not in the doc
    (0x040, "dma_put"),
    (0x044, "dma_get"),
];

/// The only BAR0 range a PTOP-derived candidate may be probed in.
///
/// Lower bound `0x20000`: below the PFIFO host window (`0x2000`) there is nothing this
/// rung has business reading, and a small decode slip lands there. Upper bound `0x100000`:
/// PFB begins there (`regs::NV_PFB_BASE`). The window therefore CANNOT contain PGRAPH
/// (`0x400000`), the FECS falcon (`0x409000`, and `0x409504` wedges the unit on first
/// touch — spec §5.4), GPCCS (`0x41A000`), or PDISPLAY (`0x610000`). Proven below, not
/// promised in prose.
const PROBE_WINDOW: (usize, usize) = (0x0002_0000, 0x0010_0000);

/// A candidate is probed only if it is window-resident AND 0x1000-aligned. Alignment is
/// not decoration: the probe offsets are added to the base with `+`, so a base carrying
/// low bits shifts the whole window and every dword read through it is a plausible-looking
/// lie about the wrong register — `kepler_ce.rs`'s `const _` makes the same argument for
/// the CE bases.
const fn candidate_admissible(base: usize) -> bool {
    base >= PROBE_WINDOW.0 && base < PROBE_WINDOW.1 && (base & 0xFFF) == 0
}

// Compile-time proofs, not comments. §5.4 cost this campaign three sittings; a comment
// saying "the window excludes the falcon" would not have stopped any of them.
const _: () = {
    // 1. The poison register, the two falcon unit bases and PGRAPH are all unreachable —
    //    stated as the addresses themselves, so the assertion survives a window edit.
    assert!(!candidate_admissible(0x0040_9000), "KFBIND may never probe the FECS falcon");
    assert!(!candidate_admissible(0x0041_A000), "KFBIND may never probe the GPCCS falcon");
    assert!(!candidate_admissible(0x0040_0000), "KFBIND may never probe PGRAPH");
    assert!(!candidate_admissible(0x0061_0000), "KFBIND may never probe PDISPLAY");
    assert!(0x0040_9504 >= PROBE_WINDOW.1, "the poison offset must lie outside the window");
    // 2. A misaligned candidate is refused.
    assert!(!candidate_admissible(0x0004_0108), "a candidate must be 0x1000-aligned");
    // 3. The base this rung is auditing IS admissible, or the rung scores nothing.
    assert!(candidate_admissible(PBDMA_LEGACY_BASE), "the legacy base must be probeable");
    assert!(
        candidate_admissible(PBDMA_LEGACY_BASE + (PBDMA_LEGACY_N - 1) * PBDMA_LEGACY_STRIDE),
        "every legacy unit must be probeable"
    );
    // 4. No probe offset is the poison offset's low half at any admissible base, and none
    //    reaches outside its own 0x1000 window.
    let mut i = 0;
    while i < PBDMA_PROBE_OFFSETS.len() {
        assert!(PBDMA_PROBE_OFFSETS[i].0 < 0x1000, "a PBDMA probe offset must stay in its window");
        i += 1;
    }
};

/// The writes this rung is named for and does not perform, each with the reason.
///
/// Printed verbatim on the wire so the capture says what was refused and why — an absent
/// line would read as a rung that simply had nothing to write.
///
/// `runlist_submit` is the exception that proves the rule: it IS citable (`0x2270/0x2274`,
/// [METAL] — `playlist_rd` echoes our own submit, s#5 onward) and it is ALREADY PERFORMED
/// by `kepler::init` between this module's two calls. Duplicating it here would submit the
/// runlist twice and make the post-read uninterpretable, so this rung brackets it instead
/// of repeating it.
const SKIPPED_WRITES: [(&str, &str); 3] = [
    (
        "pbdma_chan_bind",
        "uncited — the CHANNEL register's bind/enable bit encoding at +0x120 is named by no source in this tree (kepler.rs decodes CHID[11:0] and ACTIVE bit13 for READING only) and no rnndb file was opened for this worktree; spec 0.1 UNPINNED is probe-only",
    ),
    (
        "pbdma_start_clock",
        "uncited — s#4/s#5 asked for 'the unit start/enable/clock sequence' and nothing since has named a register for it; NV_PMC_SUBFIFO_ENABLE (0x204) is already written unconditionally by kepler::init's fifo leg and is not it",
    ),
    (
        "runlist_submit",
        "not-skipped-for-citation — 0x2270/0x2274 is METAL-proven and is performed by kepler::init BETWEEN this rung's two halves; repeating it here would double-submit and void the post-read",
    ),
];

// ===========================================================================
// Shared instrument helpers
// ===========================================================================

/// A control read of `NV_PMC_BOOT_0` at both ends of a rung (spec §5.4). Same shape and
/// same vocabulary as `kepler_ce.rs::Bracket`, deliberately: a reader comparing a KFBIND
/// capture with a CE-LADDER capture should not have to learn two idioms.
struct Bracket {
    pre: u32,
}

impl Bracket {
    fn open(bar0: usize) -> Self {
        Self { pre: unsafe { mmio_read(bar0, regs::NV_PMC_BOOT_0) } }
    }

    fn close(&self, bar0: usize) -> (u32, u32, bool) {
        let post = unsafe { mmio_read(bar0, regs::NV_PMC_BOOT_0) };
        (self.pre, post, post == self.pre)
    }
}

/// One PBDMA unit's four probe dwords, captured at a named base.
#[derive(Clone, Copy)]
struct PbdmaSample {
    base: usize,
    vals: [u32; PBDMA_PROBE_OFFSETS.len()],
}

impl PbdmaSample {
    fn read(bar0: usize, base: usize) -> Self {
        let mut vals = [0u32; PBDMA_PROBE_OFFSETS.len()];
        for (slot, (off, _)) in vals.iter_mut().zip(PBDMA_PROBE_OFFSETS.iter()) {
            *slot = unsafe { mmio_read(bar0, base + off) };
        }
        Self { base, vals }
    }

    fn status(&self) -> u32 {
        self.vals[2]
    }
    fn chan(&self) -> u32 {
        self.vals[3]
    }

    /// `true` when at least one of the four dwords is a real value — the only thing that
    /// distinguishes "this base exists" from "this address returns zero".
    fn answers(&self) -> bool {
        self.vals.iter().any(|v| classify_fecs_word(*v) == "VALUE")
    }

    fn print(&self, tag: &str, idx: usize) {
        serial_println!(
            ":: KFBIND: pbdma[{}] {} base={:06X} status={:08X} {} chan={:08X} {} ctrl_lo={:08X} {} ctrl_hi={:08X} {} answers={} — CHID/ACTIVE are DELIBERATELY NOT decoded here: at an unproven base a zero is a READ-ZERO, not a scheduler state, and printing 'CHID=0 ACTIVE=0' is the ten-sitting error this rung exists to stop ::",
            idx, tag, self.base,
            self.vals[2], classify_fecs_word(self.vals[2]),
            self.vals[3], classify_fecs_word(self.vals[3]),
            self.vals[0], classify_fecs_word(self.vals[0]),
            self.vals[1], classify_fecs_word(self.vals[1]),
            if self.answers() { "Y" } else { "n" }
        );
    }
}

/// The channel control area's five words, read through BAR1 at `userd_off`.
#[derive(Clone, Copy)]
struct UserdSample {
    vals: [u32; USERD_PROBE_OFFSETS.len()],
}

impl UserdSample {
    fn read(bar1: usize, userd_off: usize) -> Self {
        let mut vals = [0u32; USERD_PROBE_OFFSETS.len()];
        for (slot, (off, _)) in vals.iter_mut().zip(USERD_PROBE_OFFSETS.iter()) {
            *slot = unsafe { core::ptr::read_volatile((bar1 + userd_off + off) as *const u32) };
        }
        Self { vals }
    }

    /// `IB_GET` — the falsifier. Index 0 by construction; the `const _` pins it.
    fn ib_get(&self) -> u32 {
        self.vals[0]
    }
    fn ib_put(&self) -> u32 {
        self.vals[1]
    }

    fn print(&self, tag: &str, userd_off: usize) {
        serial_println!(
            ":: KFBIND: userd {} off={:08X} ib_get={:08X} {} ib_put={:08X} {} x090={:08X} {} dma_put={:08X} {} dma_get={:08X} {} — offsets from envytools docs/hw/fifo/dma-pusher.rst 'Channel control area' as quoted by kepler.rs; ib_get (0x88) is READ HERE FOR THE FIRST TIME, the deleted witness read 0x8C/0x90 which is IB_PUT and an unnamed word ::",
            tag, userd_off,
            self.vals[0], classify_fecs_word(self.vals[0]),
            self.vals[1], classify_fecs_word(self.vals[1]),
            self.vals[2], classify_fecs_word(self.vals[2]),
            self.vals[3], classify_fecs_word(self.vals[3]),
            self.vals[4], classify_fecs_word(self.vals[4])
        );
    }
}

const _: () = {
    assert!(USERD_PROBE_OFFSETS[0].0 == 0x088, "ib_get must be index 0 — the falsifier reads it");
    assert!(USERD_PROBE_OFFSETS[1].0 == 0x08C, "ib_put must be index 1");
    assert!(PBDMA_PROBE_OFFSETS[2].0 == 0x108, "status must be index 2");
    assert!(PBDMA_PROBE_OFFSETS[3].0 == 0x120, "chan must be index 3");
};

/// What `kfbind_pre` measured, carried to `kfbind_post` so the two halves compare the
/// SAME addresses. Passing the addresses forward rather than re-deriving them is the
/// point: a post-read that re-ran the decode could score a different base than the one it
/// claims to be re-reading.
pub struct KfbindPre {
    /// The PTOP-derived candidate bases that passed [`candidate_admissible`], in table
    /// order. Bounded by construction.
    derived: [usize; MAX_DERIVED],
    derived_n: usize,
    /// Pre-submit samples at the derived bases.
    derived_pre: [PbdmaSample; MAX_DERIVED],
    /// Pre-submit samples at the legacy bases.
    legacy_pre: [PbdmaSample; PBDMA_LEGACY_N],
    /// Pre-submit channel control area.
    userd_pre: UserdSample,
    userd_off: usize,
    /// `false` when the PTOP bracket moved — every derived base is then uninterpretable
    /// and the verdict says so instead of scoring them.
    ptop_bracket_held: bool,
    /// `true` when the PTOP sweep found no admissible candidate at all. Not a failure of
    /// the rung: it is the C6-layout refutation, and the legacy half still scores.
    derived_absent: bool,
}

/// Bound on candidates carried forward. The table is 64 entries; carrying every one would
/// put 64 PBDMA probe sweeps (256 BAR0 reads) and 64 lines on the wire, which is the FTDI
/// ring budget `kepler_ce.rs` already had to defend against. Eight is `PBDMA_LEGACY_N`
/// rounded to the runlist-array width `kepler_ce.rs::RL_ARRAY_N` uses; overflow is COUNTED
/// and printed, never silently dropped.
const MAX_DERIVED: usize = 8;

// ===========================================================================
// Phase A — PTOP sweep, derivation, and the pre-submit read
// ===========================================================================

/// Runs from `kepler::init`'s fifo leg immediately BEFORE the runlist rebuild + submit.
///
/// **Stimulus:** 64 BAR0 reads at PTOP, up to `MAX_DERIVED * 4` at derived bases,
/// `PBDMA_LEGACY_N * 4` at legacy bases, 5 BAR1 reads at USERD. **No writes.**
pub fn kfbind_pre(bar0: usize, bar1: usize, userd_off: usize) -> KfbindPre {
    serial_println!(
        ":: KFBIND: begin knob=UNAOS_KEPLER_KFBIND rung=KF27 subject=\"the PBDMA register base every FIFO verdict is read through\" writes=0 restored=n/a — READ-ONLY by construction; the bind/enable write this rung is named for is SKIPPED and its reason printed below ::"
    );

    // ---- 1. PTOP device-info sweep, bracketed, raw first -------------------
    let br = Bracket::open(bar0);

    let mut raw = [0u32; PTOP_ENTRIES];
    for (i, slot) in raw.iter_mut().enumerate() {
        *slot = unsafe { mmio_read(bar0, PTOP_DEVICE_INFO + i * 4) };
    }

    // 8 per line: 64 single-entry lines would evict the display and ucode legs from a
    // 64 KiB drop-oldest FTDI capture (the budget that silenced MIRROR_HDR_DENSE).
    for row in 0..(PTOP_ENTRIES / 8) {
        let b = row * 8;
        serial_println!(
            ":: KFBIND: ptop-row i={:02}..{:02} [{:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X} {:08X}] ::",
            b, b + 7,
            raw[b], raw[b + 1], raw[b + 2], raw[b + 3],
            raw[b + 4], raw[b + 5], raw[b + 6], raw[b + 7]
        );
    }

    // ---- 2. Derivation. The RAW is the datum; this is a labelled hypothesis ----
    let mut derived = [0usize; MAX_DERIVED];
    let mut derived_n = 0usize;
    let mut nonzero = 0usize;
    let mut poison = 0usize;
    let mut refused = 0usize;
    let mut overflow = 0usize;
    // The hypothesised runlist-id nibble, kept ONLY so the `runlists=[…]` token the queue
    // row asks for exists in the capture. Its bit position is a guess with no corroborating
    // observation on this part, so it is printed under an explicit tag and nothing reads it.
    let mut runlist_bits = 0u32;

    for &v in raw.iter() {
        match classify_fecs_word(v) {
            "POISON" => {
                poison += 1;
                continue;
            }
            "ZERO" => continue,
            _ => {}
        }
        nonzero += 1;
        runlist_bits |= 1u32 << ((v >> 16) & 0xF);

        let cand = ptop_pri_candidate(v);
        if !candidate_admissible(cand) {
            refused += 1;
            continue;
        }
        // Distinct bases only — a device-info table names several engines per PRI window.
        if derived[..derived_n].contains(&cand) {
            continue;
        }
        if derived_n == MAX_DERIVED {
            overflow += 1;
            continue;
        }
        derived[derived_n] = cand;
        derived_n += 1;
    }

    let (ptop_pre, ptop_post, ptop_held) = br.close(bar0);

    serial_println!(
        ":: KFBIND: ptop pbdma={} runlists=[{:04X}] pbdma0_base={:06X} entries={} nonzero={} poison={} refused_out_of_window={} overflow={} ctl_pre={:08X} ctl_post={:08X} ctl={} — pbdma= is the count of DISTINCT in-window 0x1000-aligned PRI candidates, extracted with kepler_ce.rs R1's own ((v&0xFFF)<<12); runlists= is a HYPOTHESIS(bitmask of (v>>16)&0xF, UNPINNED, no rnndb file opened) printed so the token exists, and NOTHING below reads it ::",
        derived_n,
        runlist_bits,
        if derived_n > 0 { derived[0] } else { 0 },
        PTOP_ENTRIES, nonzero, poison, refused, overflow,
        ptop_pre, ptop_post,
        if ptop_held { "held" } else { "MOVED — every dword above is uninterpretable" }
    );

    if !ptop_held {
        serial_println!(
            ":: KFBIND: ptop VOID — the NV_PMC_BOOT_0 bracket moved across the sweep. No derived base is claimed; the legacy half below still runs and is still scored, because it is read at an address this driver has used since s#3 either way ::"
        );
    } else if poison == PTOP_ENTRIES {
        serial_println!(
            ":: KFBIND: ptop REFUTED-CLEANLY — all {} entries are nonexistent-PRI faults. The device-info table is NOT at 0x022700 on this part; the derivation this rung is built on is dead and must be re-derived from a Group-A document before another boot is spent. CE-R1 (kepler_ce.rs) reads the same address and will agree or disagree in the same capture ::",
            PTOP_ENTRIES
        );
    } else if derived_n == 0 {
        serial_println!(
            ":: KFBIND: ptop NO-CANDIDATE — the table answers ({} nonzero) but no entry decodes to an in-window 0x1000-aligned PRI base ({} refused). This is evidence against the DECODE, not against the table: the raw rows above are the datum and the legacy comparison below is unaffected ::",
            nonzero, refused
        );
    }

    // ---- 3. The pre-submit read, DERIVED beside LEGACY ---------------------
    let empty = PbdmaSample { base: 0, vals: [0; PBDMA_PROBE_OFFSETS.len()] };
    let mut derived_pre = [empty; MAX_DERIVED];
    for i in 0..derived_n {
        derived_pre[i] = PbdmaSample::read(bar0, derived[i]);
        derived_pre[i].print("DERIVED pre-submit", i);
    }

    let mut legacy_pre = [empty; PBDMA_LEGACY_N];
    for (i, slot) in legacy_pre.iter_mut().enumerate() {
        *slot = PbdmaSample::read(bar0, PBDMA_LEGACY_BASE + i * PBDMA_LEGACY_STRIDE);
        slot.print("LEGACY pre-submit", i);
    }

    let userd_pre = UserdSample::read(bar1, userd_off);
    userd_pre.print("pre-submit", userd_off);

    // ---- 4. The writes this rung refuses, and why --------------------------
    for (name, reason) in SKIPPED_WRITES.iter() {
        serial_println!(":: KFBIND: skipped write={} reason={} ::", name, reason);
    }

    KfbindPre {
        derived,
        derived_n,
        derived_pre,
        legacy_pre,
        userd_pre,
        userd_off,
        ptop_bracket_held: ptop_held,
        derived_absent: derived_n == 0,
    }
}

// ===========================================================================
// Phase B — the post-submit re-read and the falsifier
// ===========================================================================

/// Runs from `kepler::init`'s fifo leg AFTER the runlist submit and its playlist-echo
/// poll, immediately after the historic `DISCRIMINATOR` loop so the two sit adjacent in
/// the capture and can be read against each other.
///
/// **The falsifier, stated before the code that scores it:** `IB_GET` (USERD +0x88)
/// advancing past its pre-submit value means the PBDMA fetched — `-> FETCHED`. Unchanged
/// means `-> STILL-DARK under <conditions>`, with the conditions NAMED (R19: never "ruled
/// out"). A moved control bracket, or a USERD that reads POISON, scores neither.
pub fn kfbind_post(bar0: usize, bar1: usize, pre: &KfbindPre) {
    let br = Bracket::open(bar0);

    for i in 0..pre.derived_n {
        let s = PbdmaSample::read(bar0, pre.derived[i]);
        s.print("DERIVED post-submit", i);
        let d = &pre.derived_pre[i];
        serial_println!(
            ":: KFBIND: delta derived[{}] base={:06X} status {:08X}->{:08X} {} chan {:08X}->{:08X} {} ::",
            i, s.base,
            d.status(), s.status(), if d.status() == s.status() { "same" } else { "MOVED" },
            d.chan(), s.chan(), if d.chan() == s.chan() { "same" } else { "MOVED" }
        );
    }

    for (i, d) in pre.legacy_pre.iter().enumerate() {
        let s = PbdmaSample::read(bar0, PBDMA_LEGACY_BASE + i * PBDMA_LEGACY_STRIDE);
        s.print("LEGACY post-submit", i);
        serial_println!(
            ":: KFBIND: delta legacy[{}] base={:06X} status {:08X}->{:08X} {} chan {:08X}->{:08X} {} ::",
            i, s.base,
            d.status(), s.status(), if d.status() == s.status() { "same" } else { "MOVED" },
            d.chan(), s.chan(), if d.chan() == s.chan() { "same" } else { "MOVED" }
        );
    }

    let userd_post = UserdSample::read(bar1, pre.userd_off);
    userd_post.print("post-submit", pre.userd_off);

    let (ctl_pre, ctl_post, held) = br.close(bar0);

    // ---- The base comparison. This is the rung's primary product -----------
    //
    // Deliberately three-valued and deliberately NOT a preference for the derived base:
    // the flight decides, not this code.
    let derived_answers = pre.derived_pre[..pre.derived_n].iter().any(|s| s.answers());
    let legacy_answers = pre.legacy_pre.iter().any(|s| s.answers());
    let base_verdict = if !pre.ptop_bracket_held {
        "VOID-BRACKET — the PTOP sweep's control read moved; no statement is made about either base"
    } else if pre.derived_absent {
        "LEGACY-ONLY — PTOP named no admissible candidate, so the two bases were never compared. The legacy base is NOT thereby confirmed: it is simply the only one this boot read"
    } else if derived_answers && !legacy_answers {
        "DERIVED-WINS — a PTOP-derived base returns real values where the legacy base returns only zeros/poison. Every PBDMA verdict in register 2 since s#6 was read at the wrong address and is RE-SCOPED, not retracted (KF24's shape)"
    } else if legacy_answers && !derived_answers {
        "LEGACY-WINS — the legacy base answers and no derived candidate does. 0x40000 + i*0x2000 is promoted from guess to observation and s#4/s#5's NEXT item is DISCHARGED"
    } else if derived_answers && legacy_answers {
        "BOTH-ANSWER — both bases return real values. They may be the same window reached two ways, or two real units; the base= fields on the lines above are what separates them and neither is refuted"
    } else {
        "NEITHER-ANSWERS — every dword at every base, derived and legacy, is ZERO or POISON with a HELD bracket. That is a statement about the whole PBDMA PRI space on this part and it is the strongest reading this rung can produce without a start/clock sequence it may not guess"
    };

    // ---- The falsifier -----------------------------------------------------
    let ib_pre = pre.userd_pre.ib_get();
    let ib_post = userd_post.ib_get();
    let ib_cls = classify_fecs_word(ib_post);
    let fetch_verdict = if !held {
        "VOID — the control bracket moved across the post-read; IB_GET is not interpretable this boot"
    } else if ib_cls == "POISON" || ib_cls == "ABSENT" {
        "VOID-USERD — IB_GET reads POISON/ABSENT, so the channel control area itself did not answer. This says nothing about whether the PBDMA fetched"
    } else if ib_post != ib_pre {
        "FETCHED — IB_GET advanced across the submit. The PBDMA fetched from the GPFIFO, and K-GPU-3's wall since July has moved for the first time"
    } else {
        "STILL-DARK under: PTOP-derived base comparison as scored above, FECS context microcode NOT resident (this boot runs no ctx ucode before the submit), CHAN_CUR/CHAN_NEXT NOT host-populated by this rung, PBDMA start/clock sequence NOT written (uncited, skipped above), runlist submitted via 0x2270/0x2274 and its playlist echo scored by kepler::init's own post-bind line. NOT 'ruled out' (R19): this names the conditions IB_GET did not move under, and IB_GET itself was read here for the first time in the campaign"
    };

    serial_println!(
        ":: KFBIND: verdict base={} ib_get {:08X}->{:08X} {} ib_put {:08X}->{:08X} derived_n={} ctl_pre={:08X} ctl_post={:08X} ctl={} writes=0 restored=n/a -> {} ::",
        base_verdict,
        ib_pre, ib_post, ib_cls,
        pre.userd_pre.ib_put(), userd_post.ib_put(),
        pre.derived_n,
        ctl_pre, ctl_post,
        if held { "held" } else { "MOVED" },
        fetch_verdict
    );

    serial_println!(
        ":: KFBIND: end rung=KF27 ptop={} base_cmp={} fetch={} — one rollup line so a silent rung is distinguishable from a quiet pass ::",
        if !pre.ptop_bracket_held { "void" } else if pre.derived_absent { "no-candidate" } else { "derived" },
        if derived_answers { "derived-answers" } else if legacy_answers { "legacy-answers" } else { "neither" },
        if !held { "void" } else if ib_post != ib_pre { "FETCHED" } else { "still-dark" }
    );
}

// KFCTXBIND lives in an INNER, separately-gated module, and that is load-bearing rather than
// tidy: this file is declared `#[cfg(feature = "nvidia-kepler-kfbind")] pub mod kepler_fifo;`
// in `gpu/mod.rs`, so every item below would otherwise compile into a KFBIND-only build and
// shift its `-Cmetadata` and its dead-code surface. Gated here, a `UNAOS_KEPLER_KFBIND=1`
// build is byte-identical to the one that shipped before this rung landed — which is the
// property the seat measures on the fold.
#[cfg(feature = "nvidia-kepler-kfctxbind")]
pub mod ctxbind {
use super::*;
// ###########################################################################
// KFCTXBIND — KF18/KF19 re-run with the FECS context ucode CENSUSED and the
// PBDMA base DERIVED. Register §2 row `KF28 kfctxbind`; queue row
// `rmbp-queue.md` §GPU LADDERS `KFBIND-CTXBIND`. Knob
// `UNAOS_KEPLER_KFCTXBIND=1` (feature `nvidia-kepler-kfctxbind`, implies
// `nvidia-kepler` + `nvidia-kepler-fifo`).
// ###########################################################################
//
// # What this rung is, and what it is NOT
//
// §2's "what would change the verdict" asks, for KF18/KF19, for one thing:
// *re-run the bind with FECS context microcode resident and running — the one
// condition never varied across ten eliminations.* This rung does **not**
// claim to satisfy that condition. It is the instrument that MEASURES it, and
// that distinction is the whole design:
//
// * There is no vendor context-switch microcode in this tree and there may
//   never be one (`falcon_microcode_spec.md`'s CLEANROOM POLICY NOTICE: no
//   proprietary firmware blob may enter the source tree). The only images this
//   driver has ever uploaded to FECS are the campaign's own ECHO / POKE /
//   heartbeat probes, which are not context-switch microcode and do not claim
//   to be.
// * So on every boot available today the census will read
//   `ctx-ucode=absent-by-construction`, and the STILL-DARK line will NAME that
//   as a standing condition instead of letting a tenth elimination be recorded
//   under a condition nobody measured. That is the R19 shape exactly: the
//   register's ten shut-outs all say "no FECS ctx ucode running" and **not one
//   capture in the campaign contains a reading that establishes it.**
//
// # The dependency on KF27, and why it is evaluated and not assumed
//
// R19: this rung's whole product is a PBDMA-state reading, so it may not be
// scored at the address KF26 has been misreading since s#6. It therefore runs
// only when the base question is RESOLVED IN FAVOUR OF A DERIVED BASE on THIS
// boot — `DERIVED-WINS` or `BOTH-ANSWER` — and otherwise prints
// `:: KFCTXBIND: skipped reason=kf27-base-unresolved ::` and stops.
//
// ⚠ **It resolves the base ITSELF, read-only, and does not call KF27.** Two
// reasons, and both are binding:
//
// 1. `KEPLER-METAL-LOG.md`'s KF28 entry forbids flying this rung on the same
//    boot as KF27. KF27's own `STILL-DARK under:` string hard-codes the
//    conditions of a `writes=0` boot; a boot carrying both rungs would have
//    KF27 print that sentence over a boot this rung had also acted in. Two
//    verdicts about one submit, one of them stale by construction.
// 2. `nvidia-kepler-kfctxbind` therefore deliberately does **not** imply
//    `nvidia-kepler-kfbind`, so `KfbindPre` may not exist at all on an armed
//    boot and the gate cannot be a function of it.
//
// The resolution below reads the SAME `PTOP_DEVICE_INFO`, the SAME
// `ptop_pri_candidate` extraction, the SAME `candidate_admissible` window and
// the SAME `PbdmaSample` offsets KF27 uses — every one of them is a module
// constant shared with KF27 above, not a second guess. What is restated is the
// sweep LOOP and the verdict LADDER, and the ladder's arm names are the same
// strings KF27 prints, so a capture from either rung is scored in one
// vocabulary. If the two ever disagree about a dword it is a code defect in
// this file, exactly as KF27's own header says of `kepler_ce.rs` R1.
//
// # Parachute discipline — identical to KF27's, and for the same reasons
//
// * **Zero device writes.** The channel CTRL enable/bind this rung is named
//   for is STILL uncited (see [`CTXBIND_SKIPPED_WRITES`]) and stays skipped.
//   `restored=` therefore reports `n/a writes=0`.
// * **Every phase bracketed** by `NV_PMC_BOOT_0`, and the bracket GATES the
//   verdict.
// * **`0x409504` is unreachable from this rung**, proven by a `const _`, and
//   every FECS read goes through `kepler::fecs_read` so the campaign's own
//   FECS access accounting sees them.
// * **`ZERO` / `POISON` are printed, never interpreted** —
//   `classify_fecs_word` is the single vocabulary.

// ===========================================================================
// KFCTXBIND register numbers — citation class AT THE DEFINITION
// ===========================================================================
//
// Same §0.1 vocabulary as the KF27 block above. Where a line names a sitting
// it is naming `falcon_microcode_spec.md` §2's own evidence column, which is
// this bench's observation of that register on this part.

/// FECS falcon unit base. **[METAL s26]** — `falcon_microcode_spec.md` §1:
/// `cpuctl=00000010`, `imemc/dmemc=00000000`, the first non-poison Falcon
/// reads of the campaign.
const FECS_BASE: usize = 0x0040_9000;

/// ⛔ `WRCMD_CMD`. **[METAL s31/s32/s34]** — the poison offset. Named here for
/// exactly one purpose: so the `const _` below can prove this rung never reads
/// it. Nothing in this module may pass it to `fecs_read`.
const FECS_WRCMD_CMD: usize = 0x0040_9504;

/// The FECS census, read-only, absolute BAR0 addresses. Every offset is a
/// `falcon_microcode_spec.md` §2 row with a sitting in its evidence column —
/// this rung introduces no new FECS offset and probes nothing unproven.
const FECS_CENSUS: [(usize, &str, &str); 10] = [
    (FECS_BASE + 0x040, "mailbox0", "s29"),
    (FECS_BASE + 0x044, "mailbox1", "s30"),
    (FECS_BASE + 0x100, "cpuctl", "s26/s28/s31/s34"),
    (FECS_BASE + 0x108, "idlestate", "s28"),
    (FECS_BASE + 0x10C, "dmactl", "s28"),
    (FECS_BASE + 0x180, "imemc", "s26/s28"),
    (FECS_BASE + 0x1C0, "dmemc", "s26/s28"),
    (FECS_BASE + 0xB00, "chan_cur", "s34-rest/s35-write-took"),
    (FECS_BASE + 0xB04, "chan_next", "s34-rest/s35-write-took"),
    (FECS_BASE + 0xC00, "engine_status", "s34-rest/s35-CHAN_VALID-not-host-assertable"),
];

/// `CPUCTL` bit 4 — `STOPPED`. **[EXT s28]** — rnndb's bit meaning for a
/// register this bench has read at rest on this part, which is what makes it
/// EXT and not UNPINNED (§0.1). `0x10` at rest = halted; `0x00` = running
/// (s30's whole heartbeat run).
const CPUCTL_STOPPED: u32 = 1 << 4;

/// Index of `cpuctl` in [`FECS_CENSUS`]. Pinned by a `const _`, because the
/// residency verdict reads this slot by number.
const FECS_CENSUS_CPUCTL: usize = 2;
/// Index of `imemc` — the second half of the residency reading.
const FECS_CENSUS_IMEMC: usize = 5;

const _: () = {
    // 1. THE POISON LAW, as an assertion rather than a promise. §5.4 cost this
    //    campaign three sittings and a comment would not have stopped any of
    //    them.
    let mut i = 0;
    while i < FECS_CENSUS.len() {
        assert!(
            FECS_CENSUS[i].0 != FECS_WRCMD_CMD,
            "KFCTXBIND may never read 0x409504 — the first access wedges the unit for the boot"
        );
        // 2. Every census address is inside the FECS unit and nowhere else.
        assert!(FECS_CENSUS[i].0 >= FECS_BASE && FECS_CENSUS[i].0 < FECS_BASE + 0x1000);
        i += 1;
    }
    assert!(FECS_CENSUS[FECS_CENSUS_CPUCTL].0 == FECS_BASE + 0x100, "cpuctl index pinned");
    assert!(FECS_CENSUS[FECS_CENSUS_IMEMC].0 == FECS_BASE + 0x180, "imemc index pinned");
    // 3. The FECS unit is NOT in KF27's probe window, so a PTOP-derived
    //    candidate can never collide with a census address. Restated here so
    //    the assertion survives an edit to either constant.
    assert!(!candidate_admissible(FECS_BASE), "the census base must be outside the probe window");
};

/// The channel instance block, in VRAM, reached through BAR1 at `inst_off`.
///
/// ⛔ **[TREE-UNAUDITED]**, every row. `kepler.rs` writes this layout under a
/// standing CLEAN-ROOM disclaimer at the writes themselves: the audit once
/// claimed for these constants was WITHDRAWN by its own author as a §5
/// Group-B violation, and nothing may call them validated until they are
/// re-derived from a Group-A source. This rung READS BACK what `kepler.rs`
/// wrote and prints the address class beside every word, so the capture says
/// "this is where OUR driver believes the field is", never "this is the field".
/// `kepler_ce.rs` R5 is the rung that would supply the Group-A layout; it is
/// `never-run`, and until it answers these offsets carry no provenance at all.
const INST_CENSUS: [(usize, &str, &str); 9] = [
    (0x008, "userd_lo", "TREE-UNAUDITED"),
    (0x00C, "userd_hi", "TREE-UNAUDITED"),
    (0x048, "gpfifo_lo", "TREE-UNAUDITED"),
    (0x04C, "gpfifo_hi_limit2", "TREE-UNAUDITED"),
    (0x084, "eng_ctx_a", "TREE-UNAUDITED"),
    (0x094, "eng_ctx_b", "TREE-UNAUDITED"),
    (0x09C, "eng_ctx_c", "TREE-UNAUDITED"),
    (0x0B8, "eng_ctx_d", "TREE-UNAUDITED"),
    (0x0E8, "chid", "TREE-UNAUDITED"),
];

/// The page-directory-base candidate. **[UNPINNED]** — `kepler_ce.rs` R5's
/// draft-C8 hypothesis that a gf100-class instance block carries a PD pointer
/// near `+0x200`, layered on draft C9. R5 has never run, so this is a guess
/// with a citation and it is READ ONLY, printed under its own doubly-labelled
/// line, and nothing in the verdict ladder reads it.
const INST_PDB_LO: usize = 0x200;
const INST_PDB_HI: usize = 0x204;

const _: () = {
    let mut i = 0;
    while i < INST_CENSUS.len() {
        assert!(INST_CENSUS[i].0 < 0x1000, "an instance-block offset must stay inside the 4 KiB page");
        i += 1;
    }
    assert!(INST_PDB_HI < 0x1000 && INST_PDB_LO < INST_PDB_HI, "the PDB probe must stay in the page");
    // The two fields the falsifier's plumbing depends on, by index.
    assert!(INST_CENSUS[0].0 == 0x008 && INST_CENSUS[1].0 == 0x00C, "userd lo/hi are indices 0/1");
};

/// The PFIFO channel table. **[TREE]** — `kepler.rs` writes and reads
/// `0x800000 + chid * 8` for channel 1 (the bind at `2. Bind and Enable
/// PFIFO_CHAN for channel 1`, the `PFIFO_CHAN[1] pre-submit` read, the
/// post-bind witness, and the `post-submit 00=/04=` pair). The STRIDE and the
/// BASE are therefore observed on this part; **the BIT ENCODING is not** — see
/// [`CTXBIND_SKIPPED_WRITES`].
const PFIFO_CHAN_TABLE: usize = 0x0080_0000;
const PFIFO_CHAN_STRIDE: usize = 8;
/// Slots swept by the encoding instrument. Bounded, and the bound is the FTDI
/// ring budget `kepler_ce.rs` already had to defend: a 128-channel sweep is 256
/// reads and 128 lines, which evicts the display and ucode legs from a 64 KiB
/// drop-oldest capture. Eight covers every CHID this driver has ever written
/// (1, 2, 3 and s13's 7) plus slot 0.
const PFIFO_CHAN_SLOTS: usize = 8;

const _: () = {
    // The sweep may not walk out of the channel table into whatever follows it.
    assert!(
        PFIFO_CHAN_TABLE + PFIFO_CHAN_SLOTS * PFIFO_CHAN_STRIDE <= PFIFO_CHAN_TABLE + 0x1000,
        "the channel-table sweep must stay inside one 4 KiB window"
    );
    // And it may never reach PGRAPH, the falcons or the poison offset.
    assert!(PFIFO_CHAN_TABLE > 0x0061_0000, "the channel table is above every unit this rung refuses");
};

/// The writes this rung is NAMED for and does not perform, each with its
/// reason and — where one exists — the observation that would pin it.
///
/// Printed verbatim on the wire, the same discipline as KF27's
/// [`SKIPPED_WRITES`]: an absent line reads as a rung that had nothing to
/// write.
const CTXBIND_SKIPPED_WRITES: [(&str, &str); 4] = [
    (
        "chan_ctrl_enable",
        "uncited — the PFIFO channel-table word's enable/bind bit encoding is named by NO source in this tree. kepler.rs writes `0xC0000000 | (inst_off >> 12)` and the FENCE leg's `stuck` gate masks `& 0xC0000000`, but neither names what bit 31 or bit 30 MEANS; the ucode block's own comment calls it only 'control bits in the top nibble'. spec 0.1 UNPINNED is probe-only. THE INSTRUMENT THAT WOULD PIN IT IS IMPLEMENTED BELOW and needs no new write: sweep the channel table and diff a slot the FIRMWARE left bound against the slot WE wrote — see the chan_ctrl lines. A boot where a firmware-bound slot reads a top nibble we never wrote CITES the encoding by observation on this part, which is exactly what promotes it from UNPINNED to a sitting",
    ),
    (
        "fecs_ctx_ucode_load",
        "uncited AND out of scope — 'FECS context microcode resident' is the condition register 2 asks KF18/KF19 to be re-run under, and no such image exists in this tree: falcon_microcode_spec.md's CLEANROOM POLICY NOTICE forbids the vendor blob outright, and the ECHO/POKE/heartbeat images this driver does upload are probes, not a context machine. This rung MEASURES the condition and refuses to fake it; the census below is that measurement",
    ),
    (
        "chan_cur_chan_next_rebind",
        "not-skipped-for-citation — KF18's writes (0x409B00 / 0x409B04) are ALREADY PERFORMED by kepler::init immediately above this rung's phase A, and s35 recorded both as TAKEN. Repeating them here would be a second stimulus on the same registers in the same boot and would make the post-read uninterpretable; this rung brackets KF18 instead of re-issuing it",
    ),
    (
        "runlist_submit",
        "not-skipped-for-citation — 0x2270/0x2274 is METAL-proven (playlist_rd echoes our own submit, s#5 onward) and is performed by kepler::init BETWEEN this rung's two halves. Repeating it would double-submit and void the IB_GET delta, which is the falsifier",
    ),
];

// ===========================================================================
// The base resolution — KF27's ladder, evaluated by this rung, read-only
// ===========================================================================

/// The six arms of KF27's base ladder, as a value rather than a string, so the
/// R19 gate is a match and not a substring test.
///
/// The `name()` strings are KF27's verdict arms shortened to their leading
/// token, deliberately: a capture from either rung is scored in ONE vocabulary
/// and `awk 'index($0,"DERIVED-WINS")'` finds both.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BaseClass {
    VoidBracket,
    LegacyOnly,
    DerivedWins,
    LegacyWins,
    BothAnswer,
    NeitherAnswers,
}

impl BaseClass {
    fn name(self) -> &'static str {
        match self {
            BaseClass::VoidBracket => "VOID-BRACKET",
            BaseClass::LegacyOnly => "LEGACY-ONLY",
            BaseClass::DerivedWins => "DERIVED-WINS",
            BaseClass::LegacyWins => "LEGACY-WINS",
            BaseClass::BothAnswer => "BOTH-ANSWER",
            BaseClass::NeitherAnswers => "NEITHER-ANSWERS",
        }
    }

    /// R19's gate, stated once. Only these two arms say "a DERIVED base
    /// returns real values on this boot", which is the precondition for
    /// reading any PBDMA state through one.
    fn derived_usable(self) -> bool {
        matches!(self, BaseClass::DerivedWins | BaseClass::BothAnswer)
    }
}

/// What the resolution found, carried to the census so the same base is used
/// throughout rather than re-derived per phase.
struct BaseResolution {
    class: BaseClass,
    /// The derived base the rung will read PBDMA state at: the FIRST
    /// admissible candidate that ANSWERS. Zero when the class is not usable.
    base: usize,
    derived_n: usize,
    legacy_answers: bool,
}

/// KF27's derivation and ladder, read-only, on this boot.
///
/// Every constant is KF27's own. The loop is restated rather than factored out
/// of `kfbind_pre` because KF27's rung is frozen: it is the register's
/// never-flown KF27 row, and a refactor that changed one byte of its emitted
/// lines would retire a capture format the METAL log's PENDING entry already
/// quotes.
fn resolve_base(bar0: usize) -> BaseResolution {
    let br = Bracket::open(bar0);

    let mut derived = [0usize; MAX_DERIVED];
    let mut derived_n = 0usize;
    let mut nonzero = 0usize;
    let mut poison = 0usize;
    let mut refused = 0usize;

    for i in 0..PTOP_ENTRIES {
        let v = unsafe { mmio_read(bar0, PTOP_DEVICE_INFO + i * 4) };
        match classify_fecs_word(v) {
            "POISON" => {
                poison += 1;
                continue;
            }
            "ZERO" => continue,
            _ => {}
        }
        nonzero += 1;
        let cand = ptop_pri_candidate(v);
        if !candidate_admissible(cand) {
            refused += 1;
            continue;
        }
        if derived[..derived_n].contains(&cand) || derived_n == MAX_DERIVED {
            continue;
        }
        derived[derived_n] = cand;
        derived_n += 1;
    }

    let (pre, post, held) = br.close(bar0);

    // Score the two populations. `answers()` is KF27's own test: at least one
    // dword that is neither ZERO nor POISON — the only thing separating "this
    // base exists" from "this address returned zero".
    let mut derived_base = 0usize;
    let mut derived_answers = false;
    for i in 0..derived_n {
        let s = PbdmaSample::read(bar0, derived[i]);
        serial_println!(
            ":: KFCTXBIND: base-probe DERIVED[{}] base={:06X} status={:08X} {} chan={:08X} {} ctrl_lo={:08X} {} ctrl_hi={:08X} {} answers={} — CHID/ACTIVE deliberately NOT decoded (KF27's rule: a zero at an unproven base is a READ-ZERO, not a scheduler state) ::",
            i, s.base,
            s.vals[2], classify_fecs_word(s.vals[2]),
            s.vals[3], classify_fecs_word(s.vals[3]),
            s.vals[0], classify_fecs_word(s.vals[0]),
            s.vals[1], classify_fecs_word(s.vals[1]),
            if s.answers() { "Y" } else { "n" }
        );
        if s.answers() && !derived_answers {
            derived_answers = true;
            derived_base = s.base;
        }
    }

    let mut legacy_answers = false;
    for i in 0..PBDMA_LEGACY_N {
        let s = PbdmaSample::read(bar0, PBDMA_LEGACY_BASE + i * PBDMA_LEGACY_STRIDE);
        serial_println!(
            ":: KFCTXBIND: base-probe LEGACY[{}] base={:06X} status={:08X} {} chan={:08X} {} ctrl_lo={:08X} {} ctrl_hi={:08X} {} answers={} ::",
            i, s.base,
            s.vals[2], classify_fecs_word(s.vals[2]),
            s.vals[3], classify_fecs_word(s.vals[3]),
            s.vals[0], classify_fecs_word(s.vals[0]),
            s.vals[1], classify_fecs_word(s.vals[1]),
            if s.answers() { "Y" } else { "n" }
        );
        legacy_answers |= s.answers();
    }

    let class = if !held {
        BaseClass::VoidBracket
    } else if derived_n == 0 {
        BaseClass::LegacyOnly
    } else if derived_answers && !legacy_answers {
        BaseClass::DerivedWins
    } else if legacy_answers && !derived_answers {
        BaseClass::LegacyWins
    } else if derived_answers && legacy_answers {
        BaseClass::BothAnswer
    } else {
        BaseClass::NeitherAnswers
    };

    serial_println!(
        ":: KFCTXBIND: base class={} derived_n={} derived_base={:06X} legacy_answers={} entries={} nonzero={} poison={} refused_out_of_window={} ctl_pre={:08X} ctl_post={:08X} ctl={} — this is KF27's OWN ladder re-evaluated read-only on THIS boot, over KF27's own PTOP address, extraction, window and probe offsets. It is NOT a KF27 flight: KF27's knob may be off and its lines absent, and the two rungs are forbidden to fly together (a boot carrying both would have KF27 print its writes=0 condition string over a boot this rung also acted in) ::",
        class.name(), derived_n, derived_base,
        if legacy_answers { "Y" } else { "n" },
        PTOP_ENTRIES, nonzero, poison, refused,
        pre, post,
        if held { "held" } else { "MOVED — every dword above is uninterpretable" }
    );

    BaseResolution {
        class,
        base: if class.derived_usable() { derived_base } else { 0 },
        derived_n,
        legacy_answers,
    }
}

// ===========================================================================
// The precondition census — read-only, and the point of the rung
// ===========================================================================

/// The FECS residency reading, and the one thing it is allowed to conclude.
///
/// Returns the verdict token that the STILL-DARK line will name as a
/// condition. Note what is NOT here: there is no arm that returns "resident".
/// That is not an oversight and it is not pessimism — it is the honest state of
/// the tree, and an arm that could print `resident` from these registers would
/// be claiming that a running falcon is a running CONTEXT MACHINE, which is
/// precisely the inference §2's ten shut-outs have been making without a
/// reading for ten sittings.
fn fecs_ctx_residency(vals: &[u32; FECS_CENSUS.len()], held: bool) -> &'static str {
    if !held {
        return "void-bracket";
    }
    let cpuctl = vals[FECS_CENSUS_CPUCTL];
    let imemc = vals[FECS_CENSUS_IMEMC];
    if classify_fecs_word(cpuctl) == "POISON" || classify_fecs_word(imemc) == "POISON" {
        return "void-poisoned";
    }
    let halted = (cpuctl & CPUCTL_STOPPED) != 0;
    if halted {
        // cpuctl=0x10 is §2's documented REST value on this part (s26/s31/s34):
        // a halted core. Nothing is executing, so nothing is resident and
        // running, whatever IMEM happens to hold.
        "absent-by-construction-halted"
    } else {
        // The core is executing. On this tree that can only be one of the
        // campaign's own probe images — ECHO, POKE or the heartbeat — because
        // no other image exists to upload. Named as such rather than as
        // "resident": a running falcon is not a running context machine.
        "absent-by-construction-running-probe-image"
    }
}

/// Everything phase A measured, carried to phase B so the two halves compare
/// the SAME addresses and so the falsifier scores the SAME slots.
pub struct KfctxbindPre {
    /// `false` when the R19 gate refused. Phase B then prints one line.
    armed: bool,
    base_class: &'static str,
    derived_base: usize,
    ctx_resident: &'static str,
    /// The channel-table words, word0 per slot, pre-submit.
    chan_pre: [u32; PFIFO_CHAN_SLOTS],
    /// Our channel's slot, so phase B names the same one.
    our_chid: usize,
    /// Pre-submit PBDMA sample at the DERIVED base.
    pbdma_pre: PbdmaSample,
    /// Pre-submit channel control area — `IB_GET` is index 0.
    userd_pre: UserdSample,
    userd_off: usize,
}

/// One channel-table slot, both dwords.
fn chan_slot(bar0: usize, i: usize) -> (u32, u32) {
    let a = PFIFO_CHAN_TABLE + i * PFIFO_CHAN_STRIDE;
    unsafe { (mmio_read(bar0, a), mmio_read(bar0, a + 4)) }
}

// ===========================================================================
// Phase A — the gate, the census, and the pre-submit baseline
// ===========================================================================

/// Runs from `kepler::init`'s fifo leg immediately BEFORE the runlist rebuild
/// and submit, and AFTER KF18's `CHAN_CUR`/`CHAN_NEXT` bind — that ordering is
/// a contract, not a convenience. The census must read the FECS state KF18
/// actually left behind, and the `IB_GET` baseline must be taken before the
/// runlist page is handed over or the falsifier scores nothing.
///
/// **Stimulus:** `PTOP_ENTRIES` + `(derived_n + 3) * 4` BAR0 reads for the base
/// resolution, 10 FECS reads, `PFIFO_CHAN_SLOTS * 2` channel-table reads, and
/// `INST_CENSUS.len() + 2 + 5` BAR1 reads. **No writes.**
pub fn kfctxbind_pre(
    bar0: usize,
    bar1: usize,
    inst_off: usize,
    userd_off: usize,
    our_chid: usize,
) -> KfctxbindPre {
    serial_println!(
        ":: KFCTXBIND: begin knob=UNAOS_KEPLER_KFCTXBIND rung=KF28 subject=\"KF18/KF19 re-run with the FECS ctx-ucode precondition MEASURED and the PBDMA base DERIVED\" writes=0 restored=n/a depends=KF27-base-resolution ::"
    );

    let empty_pbdma = PbdmaSample { base: 0, vals: [0; PBDMA_PROBE_OFFSETS.len()] };
    let empty_userd = UserdSample { vals: [0; USERD_PROBE_OFFSETS.len()] };

    // ---- 0. The R19 gate. Nothing below runs at an unresolved base ---------
    let res = resolve_base(bar0);
    if !res.class.derived_usable() {
        serial_println!(
            ":: KFCTXBIND: skipped reason=kf27-base-unresolved class={} derived_n={} legacy_answers={} — R19: this rung's entire product is a PBDMA state reading, and KF26 has been reading that state at an unproven address since s#6. Scoring KF18/KF19 here would add an eleventh elimination at the same unproven base as the first ten. NOT a statement about the bind, the channel, or the FECS ucode: nothing was measured ::",
            res.class.name(), res.derived_n,
            if res.legacy_answers { "Y" } else { "n" }
        );
        return KfctxbindPre {
            armed: false,
            base_class: res.class.name(),
            derived_base: 0,
            ctx_resident: "not-measured",
            chan_pre: [0; PFIFO_CHAN_SLOTS],
            our_chid,
            pbdma_pre: empty_pbdma,
            userd_pre: empty_userd,
            userd_off,
        };
    }

    // ---- 1. Census part one: the FECS ctx-ucode precondition ---------------
    let br = Bracket::open(bar0);
    let mut fvals = [0u32; FECS_CENSUS.len()];
    for (slot, (addr, _, _)) in fvals.iter_mut().zip(FECS_CENSUS.iter()) {
        // `fecs_read`, never a bare `read_volatile`: it is the accessor that keeps this
        // campaign's own FECS access accounting (`FECS_ACCESS_COUNT`, `FECS_FIRST_OFFSET`
        // and the `0x409504` touch flags) honest, and a census that bypassed it would leave
        // the boot's bookkeeping under-counting by ten.
        *slot = crate::drivers::gpu::kepler::fecs_read(bar0, *addr);
    }
    let (fpre, fpost, fheld) = br.close(bar0);

    for (v, (addr, name, ev)) in fvals.iter().zip(FECS_CENSUS.iter()) {
        serial_println!(
            ":: KFCTXBIND: fecs {}={:08X} {} addr={:06X} cls=[METAL {}] ::",
            name, v, classify_fecs_word(*v), addr, ev
        );
    }

    let ctx_resident = fecs_ctx_residency(&fvals, fheld);
    serial_println!(
        ":: KFCTXBIND: ctx-ucode={} cpuctl={:08X} stopped={} imemc={:08X} ctl_pre={:08X} ctl_post={:08X} ctl={} — THERE IS NO 'resident' ARM and that is the finding, not a gap: no context-switch microcode exists in this tree to be resident (falcon_microcode_spec.md CLEANROOM POLICY NOTICE forbids the vendor blob; the ECHO/POKE/heartbeat images this driver uploads are probes, not a context machine). Register 2's ten shut-outs are all recorded under 'no FECS ctx ucode running' and NOT ONE CAPTURE IN THE CAMPAIGN CONTAINS A READING THAT ESTABLISHES IT. This line is that reading ::",
        ctx_resident,
        fvals[FECS_CENSUS_CPUCTL],
        if (fvals[FECS_CENSUS_CPUCTL] & CPUCTL_STOPPED) != 0 { "Y" } else { "n" },
        fvals[FECS_CENSUS_IMEMC],
        fpre, fpost,
        if fheld { "held" } else { "MOVED — the census above is uninterpretable" }
    );

    // ---- 2. Census part two: the channel's instance block, read back -------
    //
    // BAR1, not PRAMIN: KF24 proved BAR1 offsets ARE physical VRAM addresses on
    // this part (`bar1-identity VERDICT IDENTITY`, s43 Boot A), so this reads
    // the same bytes `kepler_ce.rs` R5 would window to — without R5's one
    // PRAMIN-window write, and without needing `nvidia-kepler-ce` armed.
    let ibr = Bracket::open(bar0);
    for (off, name, cls) in INST_CENSUS.iter() {
        let v = unsafe { core::ptr::read_volatile((bar1 + inst_off + off) as *const u32) };
        serial_println!(
            ":: KFCTXBIND: inst {}={:08X} {} off={:03X} cls=[{}] — read back from BAR1 at inst_off, the layout kepler.rs WROTE under its standing clean-room disclaimer. This says what our driver put there, never what the field IS; kepler_ce.rs R5 is the rung that would supply a Group-A layout and it is never-run ::",
            name, v, classify_fecs_word(v), off, cls
        );
    }
    let pdb_lo = unsafe { core::ptr::read_volatile((bar1 + inst_off + INST_PDB_LO) as *const u32) };
    let pdb_hi = unsafe { core::ptr::read_volatile((bar1 + inst_off + INST_PDB_HI) as *const u32) };
    let (ipre, ipost, iheld) = ibr.close(bar0);
    serial_println!(
        ":: KFCTXBIND: inst pdb +200={:08X} {} +204={:08X} {} cls=[UNPINNED C8+C9, layered] ctl_pre={:08X} ctl_post={:08X} ctl={} — kepler.rs writes NOTHING at +0x200, so a non-zero here is firmware residue or a wrong offset, never our page directory. Printed on its own doubly-labelled line so a wrong guess cannot contaminate the rows above ::",
        pdb_lo, classify_fecs_word(pdb_lo),
        pdb_hi, classify_fecs_word(pdb_hi),
        ipre, ipost,
        if iheld { "held" } else { "MOVED — the instance-block rows above are uninterpretable" }
    );

    // ---- 3. The encoding instrument — the chan_ctrl sweep ------------------
    //
    // This is what `chan_ctrl_enable` is skipped FOR. The write needs the bit
    // encoding; the encoding needs an observation on this part; the observation
    // is a diff between a slot the FIRMWARE bound and the slot WE wrote. Both
    // exist in this boot already — kepler.rs bound CHID 1 far above — so the
    // instrument is a read, not a write, and it ships armed.
    let cbr = Bracket::open(bar0);
    let mut chan_pre = [0u32; PFIFO_CHAN_SLOTS];
    let (ours_w0, ours_w1) = chan_slot(bar0, our_chid.min(PFIFO_CHAN_SLOTS - 1));
    for (i, slot) in chan_pre.iter_mut().enumerate() {
        let (w0, w1) = chan_slot(bar0, i);
        *slot = w0;
        serial_println!(
            ":: KFCTXBIND: chan_ctrl[{}] fw-bound=0x{:08X} ours=0x{:08X} w1=0x{:08X} {} nibble_diff=0x{:X} self={} — fw-bound is THIS slot's word0 and ours is CHID {}'s, side by side so the top-nibble encoding is pinned BY DIFF and not by recollection. A slot we never wrote whose top nibble is non-zero CITES the enable encoding on this part (spec 0.1: that promotes it from UNPINNED to a sitting) and is what the skipped chan_ctrl_enable write is waiting for ::",
            i, *slot, ours_w0, w1, classify_fecs_word(*slot),
            (*slot >> 28) ^ (ours_w0 >> 28),
            if i == our_chid { "Y" } else { "n" },
            our_chid
        );
    }
    let (cpre, cpost, cheld) = cbr.close(bar0);
    let fw_bound_slots = chan_pre
        .iter()
        .enumerate()
        .filter(|(i, v)| *i != our_chid && classify_fecs_word(**v) == "VALUE")
        .count();
    serial_println!(
        ":: KFCTXBIND: chan_ctrl census slots={} ours_chid={} ours=0x{:08X} w1=0x{:08X} fw_bound_slots={} ctl_pre={:08X} ctl_post={:08X} ctl={} — fw_bound_slots counts slots that are NOT ours and read a real value. ZERO of them means the firmware left no bound channel in the first {} slots and the encoding STAYS UNPINNED this boot; the rung then reports the write as skipped, which is the honest outcome and not a failure of the instrument ::",
        PFIFO_CHAN_SLOTS, our_chid, ours_w0, ours_w1, fw_bound_slots,
        cpre, cpost,
        if cheld { "held" } else { "MOVED" },
        PFIFO_CHAN_SLOTS
    );

    // ---- 4. The pre-submit baseline, at the DERIVED base -------------------
    let pbdma_pre = PbdmaSample::read(bar0, res.base);
    serial_println!(
        ":: KFCTXBIND: pbdma pre-submit base={:06X} status={:08X} {} chan={:08X} {} ctrl_lo={:08X} {} ctrl_hi={:08X} {} — read at the DERIVED base this boot resolved ({}), which is the entire reason this rung is sequenced behind KF27 ::",
        pbdma_pre.base,
        pbdma_pre.vals[2], classify_fecs_word(pbdma_pre.vals[2]),
        pbdma_pre.vals[3], classify_fecs_word(pbdma_pre.vals[3]),
        pbdma_pre.vals[0], classify_fecs_word(pbdma_pre.vals[0]),
        pbdma_pre.vals[1], classify_fecs_word(pbdma_pre.vals[1]),
        res.class.name()
    );

    let userd_pre = UserdSample::read(bar1, userd_off);
    serial_println!(
        ":: KFCTXBIND: userd pre-submit off={:08X} ib_get={:08X} {} ib_put={:08X} {} x090={:08X} dma_put={:08X} dma_get={:08X} — IB_GET (0x88) is the falsifier, from envytools docs/hw/fifo/dma-pusher.rst 'Channel control area' as quoted by kepler.rs ::",
        userd_off,
        userd_pre.vals[0], classify_fecs_word(userd_pre.vals[0]),
        userd_pre.vals[1], classify_fecs_word(userd_pre.vals[1]),
        userd_pre.vals[2], userd_pre.vals[3], userd_pre.vals[4]
    );

    // ---- 5. The writes this rung refuses, and why --------------------------
    for (name, reason) in CTXBIND_SKIPPED_WRITES.iter() {
        serial_println!(":: KFCTXBIND: skipped write={} reason={} ::", name, reason);
    }

    KfctxbindPre {
        armed: true,
        base_class: res.class.name(),
        derived_base: res.base,
        ctx_resident,
        chan_pre,
        our_chid,
        pbdma_pre,
        userd_pre,
        userd_off,
    }
}

// ===========================================================================
// Phase B — the post-submit re-read and the falsifier
// ===========================================================================

/// Runs AFTER the runlist submit and its playlist-echo poll, below the historic
/// `DISCRIMINATOR` loop and below KF27's phase B if that rung is also compiled.
///
/// **The falsifier, stated before the code that scores it:** `IB_GET`
/// (USERD +0x88) advancing past its pre-submit value means the PBDMA fetched —
/// `-> FETCHED`. Unchanged means `-> STILL-DARK under {ctx-ucode=…,
/// base=derived, chan_ctrl=…}` with every condition NAMED (R19: never "ruled
/// out"). A moved control bracket, or a `IB_GET` that reads POISON, scores
/// neither.
pub fn kfctxbind_post(bar0: usize, bar1: usize, pre: &KfctxbindPre) {
    if !pre.armed {
        serial_println!(
            ":: KFCTXBIND: end rung=KF28 skipped=kf27-base-unresolved base_class={} ctx-ucode={} fetch=not-scored writes=0 restored=n/a — one rollup line even when the gate refused, so a silent rung is distinguishable from a quiet pass ::",
            pre.base_class, pre.ctx_resident
        );
        return;
    }

    let br = Bracket::open(bar0);

    let pbdma_post = PbdmaSample::read(bar0, pre.derived_base);
    serial_println!(
        ":: KFCTXBIND: pbdma post-submit base={:06X} status {:08X}->{:08X} {} chan {:08X}->{:08X} {} — CHID/ACTIVE deliberately NOT decoded ::",
        pbdma_post.base,
        pre.pbdma_pre.status(), pbdma_post.status(),
        if pre.pbdma_pre.status() == pbdma_post.status() { "same" } else { "MOVED" },
        pre.pbdma_pre.chan(), pbdma_post.chan(),
        if pre.pbdma_pre.chan() == pbdma_post.chan() { "same" } else { "MOVED" }
    );

    let mut chan_moved = 0usize;
    for (i, before) in pre.chan_pre.iter().enumerate() {
        let (w0, w1) = chan_slot(bar0, i);
        if w0 != *before {
            chan_moved += 1;
        }
        serial_println!(
            ":: KFCTXBIND: chan_ctrl[{}] post 0x{:08X}->0x{:08X} {} w1=0x{:08X} self={} ::",
            i, *before, w0,
            if w0 == *before { "same" } else { "MOVED" },
            w1,
            if i == pre.our_chid { "Y" } else { "n" }
        );
    }

    let userd_post = UserdSample::read(bar1, pre.userd_off);
    serial_println!(
        ":: KFCTXBIND: userd post-submit off={:08X} ib_get={:08X} {} ib_put={:08X} {} x090={:08X} dma_put={:08X} dma_get={:08X} ::",
        pre.userd_off,
        userd_post.vals[0], classify_fecs_word(userd_post.vals[0]),
        userd_post.vals[1], classify_fecs_word(userd_post.vals[1]),
        userd_post.vals[2], userd_post.vals[3], userd_post.vals[4]
    );

    let (ctl_pre, ctl_post, held) = br.close(bar0);

    let ib_pre = pre.userd_pre.ib_get();
    let ib_post = userd_post.ib_get();
    let ib_cls = classify_fecs_word(ib_post);

    let fetch_verdict = if !held {
        "VOID — the control bracket moved across the post-read; IB_GET is not interpretable this boot"
    } else if ib_cls == "POISON" || ib_cls == "ABSENT" {
        "VOID-USERD — IB_GET reads POISON/ABSENT, so the channel control area itself did not answer. This says nothing about whether the PBDMA fetched"
    } else if ib_post != ib_pre {
        "FETCHED — IB_GET advanced across the submit at the DERIVED base, with the FECS ctx-ucode precondition MEASURED rather than assumed. K-GPU-3's wall since July has moved"
    } else {
        "STILL-DARK"
    };

    // The conditions, named. R19 forbids "ruled out", and it also forbids a
    // condition list written from memory: every token below is a value this
    // boot actually read, printed beside the verdict that depends on it.
    serial_println!(
        ":: KFCTXBIND: verdict base={} derived_base={:06X} ctx-ucode={} chan_ctrl=skipped-uncited ib_get {:08X}->{:08X} {} ib_put {:08X}->{:08X} chan_slots_moved={} ctl_pre={:08X} ctl_post={:08X} ctl={} writes=0 restored=n/a -> {} under {{ctx-ucode={}, base={}@{:06X}, chan_ctrl=skipped, kf18_bind=performed-by-kepler::init-above, runlist=submitted-0x2270/0x2274-between-the-halves}} ::",
        pre.base_class, pre.derived_base, pre.ctx_resident,
        ib_pre, ib_post, ib_cls,
        pre.userd_pre.ib_put(), userd_post.ib_put(),
        chan_moved,
        ctl_pre, ctl_post,
        if held { "held" } else { "MOVED" },
        fetch_verdict,
        pre.ctx_resident, pre.base_class, pre.derived_base
    );

    serial_println!(
        ":: KFCTXBIND: end rung=KF28 base={} ctx-ucode={} chan_ctrl=skipped fetch={} — one rollup line so a silent rung is distinguishable from a quiet pass. READ IT WITH THE ctx-ucode TOKEN: a still-dark here is the ELEVENTH elimination only if that token ever reads something other than absent-by-construction, and today it cannot ::",
        pre.base_class, pre.ctx_resident,
        if !held { "void" } else if ib_post != ib_pre { "FETCHED" } else { "still-dark" }
    );
}
} // mod ctxbind

// KFUNWEDGE lives in an INNER, separately-gated module for the reason `mod ctxbind` above does:
// this file is declared in `gpu/mod.rs` under a gate that now names three features, so every item
// below would otherwise compile into a KFBIND-only or KFCTXBIND-only build and shift its
// `-Cmetadata` and its dead-code surface. Gated here, both of those builds stay byte-identical to
// the ones that shipped before this rung landed — the property the seat measures on the fold.
#[cfg(feature = "nvidia-kepler-kfunwedge")]
pub mod unwedge {
use super::*;
// ###########################################################################
// KFUNWEDGE — the un-wedge experiment. Register §2 row `KF29 kfunwedge`; queue
// row `rmbp-queue.md` §GPU LADDERS `KFUNWEDGE`. Knob
// `UNAOS_KEPLER_KFUNWEDGE=1` (feature `nvidia-kepler-kfunwedge`, implies
// `nvidia-kepler` + `nvidia-kepler-fifo`).
// ###########################################################################
//
// # ⚠⚠ THIS IS A SACRIFICIAL BOOT. READ THIS PARAGRAPH BEFORE ARMING IT.
//
// The rung DELIBERATELY performs the one access this campaign has spent three
// sittings learning not to perform: a HOST READ of `0x409504` (`WRCMD_CMD`).
// `falcon_microcode_spec.md` §5.4 — the poison law — says what that does:
// *the first access faults immediately and wedges every subsequent read in the
// FECS unit for the rest of the boot.* s31 discovered it, s32 proved it with
// its own control frame (`recon-pre cpuctl=00000000` real, `recon-post
// cpuctl=BADF1000`, the SAME register microseconds apart), s34 convicted the
// offset by elimination.
//
// Consequences the operator must accept BEFORE arming the knob:
//
// * **This boot's display may not survive.** On the rMBP the Kepler IS the
//   panel: the x86 compositor's ignition is the Kepler takeover. A wedged PRI
//   ring on the GPU driving the screen may take the panel with it, and the
//   recovery is a POWER CYCLE, not a reboot. Fly it with the serial capture as
//   the deliverable and expect nothing from the glass.
// * **A poison is NOT RESTORABLE.** Every other rung in this file reports
//   `writes=0 restored=n/a` because it wrote nothing. This one writes — a W1C
//   of latched fault bits — and still reports the poison itself as
//   `restored=IMPOSSIBLE`, because there is no write that un-reads a read. The
//   honest bound is the power cycle, and saying "restored" of anything here
//   would be a success echo that cannot fail.
// * **FLY IT LAST, ALONE.** Never on the same boot as BEAMX86, KDHEAD, KF27
//   (`UNAOS_KEPLER_KFBIND`) or KF28 (`UNAOS_KEPLER_KFCTXBIND`). Every verdict
//   collected after this rung is void by §5.4, and every rung listed carries
//   its own conditions string that would be printed over a boot this rung had
//   already poisoned. The call site enforces the ORDERING (it is the last
//   kepler statement before the terminal poke); it cannot enforce the operator's
//   knob set, so the knob set is the operator's discipline and it is written on
//   the queue row, the register row and the metal log entry as well as here.
// * **The terminal poke's own datum is VOID on a KFUNWEDGE boot.** §10 of the
//   spec says the terminal `fecs_write(0x409504, 0)` is evidenced by being the
//   FIRST and LAST access to that offset in the boot. This rung reads it first.
//   The `fecs-ledger` line prints `504_read_idx=` and `504_write_idx=` so the
//   capture states the ordering rather than leaving a reader to assume it.
//
// # What has never been done, and why this rung is it
//
// §2's KF21 row is explicit, and it has stood unexercised for ten sittings:
//
// > **the poison register is writable without consequence to the boot.**
// > Reading it first is what poisons; writing it last is harmless. The un-wedge
// > experiment ("does a PRING clear recover the unit?") remains **UNEXERCISED**
// > — nothing has ever wedged on a boot that went looking.
//
// The instrument for it was designed and LANDED once, at pull 30 (`0e26447e`):
// a safest-first chain over the five unknown CTXCTL offsets that would, on the
// first `BADF`, read the PRING fault registers, W1C them, and re-read `cpuctl`.
// It flew at s34 and **all five offsets read clean**, so the un-wedge half
// never executed and the code was later removed. s33boot1 recorded the same
// miss in one sentence: *"nothing wedged this boot, so 'PRING clear recovers
// the unit' was not exercised; it needs a boot where the poison deliberately
// fires."*
//
// So the missing ingredient was never the clear. It was a wedge that fires ON
// PURPOSE, at a point in the boot where losing the unit costs nothing. That is
// this rung, and it is the entire content of it.
//
// # The three questions one boot of this answers
//
// 1. **Does the fault reach the PRI ring's own reporting registers?** s33 read
//    `PBUS_INTR=0x0000000C` — bits 2 and 3 latched — with all three PIBUS fault
//    registers zero, on a boot where NOTHING of ours faulted. That reading has
//    never been taken across a fault we caused. If PBUS_INTR moves here, the
//    `0x0C` at s33 is explained and the register is promoted from "latched
//    something, meaning TBD" to an instrument.
// 2. **Is the poison the FECS unit's, or the endpoint's?** s31 says PFIFO
//    (`0x2xxx`) is unaffected, but that was inferred from a witness line, not
//    from a control read taken in the same breath as a poisoned `cpuctl`. This
//    rung reads `NV_PMC_BOOT_0` immediately after the poison. A real chip ID
//    beside a `BADF1000` `cpuctl` bounds the damage to the unit; a poisoned
//    chip ID says the whole BAR0 path is down and the boot's remaining lines
//    are worthless.
// 3. **Does a W1C of the latched fault bits recover the unit?** The answer is
//    the rung's verdict and it has three arms, none of them a guess: see
//    [`kfunwedge`]'s tail.
//
// # Parachute discipline
//
// * **Exactly ONE read of `0x409504`, and it goes through `kepler::fecs_read`**
//   so the campaign's own access ledger (`FECS_ACCESS_COUNT`,
//   `FECS_504_READ_TOUCHED`, `FECS_504_READ_INDEX`) counts it. A `const _`
//   below pins the offset, and the baseline census provably cannot contain it.
// * **The only writes are W1C write-backs of bits this boot READ AS SET**, in
//   the two registers pull 30 named. Writing back the exact value just read is
//   the least-assumptive write available: in a W1C register it clears precisely
//   the latched bits and nothing else, and in a plain RW register it is a
//   no-op by construction. A register reading ZERO is not written at all —
//   there is nothing latched to clear and a zero write would assert a meaning
//   for a field this bench has not observed.
// * **Bracketed** by `NV_PMC_BOOT_0` like every other rung here, and the
//   bracket GATES the verdict. Note the bracket is doing real work in this rung
//   rather than riding along: it is also question 2's control.
// * **`ZERO` / `POISON` printed, never interpreted** — `classify_fecs_word` is
//   the single vocabulary, shared with KF27, KF28 and `kepler_ce.rs`.
// * **R19 gate on KF27**, same as KF28: the rung does not run at an unresolved
//   PBDMA base. See [`kf27_base_class`].

// ===========================================================================
// Register numbers — citation class AT THE DEFINITION
// ===========================================================================
//
// Same `falcon_microcode_spec.md` §0.1 vocabulary the two rungs above use:
//   (sitting id) observed on this bench · DERIVED follows from a proven rule ·
//   EXT external doc for a register this bench HAS ALSO observed ·
//   UNPINNED external or inferred with NO corroborating observation — probe
//   only, never the sole basis for a write.
//
// ⚠ NO rnndb FILE WAS OPENED FOR THIS MODULE either. Every external spelling
// below is quoted from a document IN THIS TREE that quotes envytools —
// `docs/dev/GEMINI/video/Kepler/PROPOSAL-kepler-fence-pull29.md` and
// `PROPOSAL-kepler-fence-pull30.md` — and every one of the four fault
// registers was READ ON THIS PART at s33boot1, which is what makes them EXT
// (a name for a register this bench has also observed) and not UNPINNED.

/// ⛔ `WRCMD_CMD`, the poison offset. **[METAL s31/s32/s34]** —
/// `falcon_microcode_spec.md` §2 (`+0x504`, "⛔ FAULTS — poisons the unit") and
/// §5.4. This is the ONE address this rung exists to touch, and the `const _`
/// below pins it out of the baseline census so the census cannot fire it early.
const FECS_WRCMD_CMD: usize = 0x0040_9504;

/// FECS `CPUCTL`. **[METAL s26/s28/s31/s34]** — `falcon_microcode_spec.md` §2
/// (`+0x100`, rest `0x00000010`). The s32 control frame's register: `recon-pre`
/// and `recon-post` were both this address, and the pair is the in-boot proof
/// of the poison law. It is the subject of this rung's before/after.
const FECS_CPUCTL: usize = 0x0040_9100;

/// FECS `MAILBOX0`. **[METAL s29]** — `falcon_microcode_spec.md` §2 (`+0x040`;
/// `ucode-post off=040 val=F00DFACE SENTINEL`). A SECOND in-unit address, read
/// beside `cpuctl` so "the unit is poisoned" is two readings and not one.
const FECS_MAILBOX0: usize = 0x0040_9040;

/// `PBUS_INTR`. **[METAL s33]** — read `0x0000000C` on this part at s33boot1
/// (`:: kepler: recon PBUS_INTR=0000000C ::`, KEPLER-METAL-LOG) and W1C'd in
/// the same boot, so both the register AND the write-back have been exercised
/// here. The ADDRESS spelling is **[EXT]** (envytools `docs/hw/bus/pbus.rst`,
/// "PBUS interrupts", as quoted by PROPOSAL-kepler-fence-pull29 §"PRING").
const PBUS_INTR: usize = 0x0000_1100;

/// `PBUS_INTR` bit 2 — `MMIO_RING_ERR`, "MMIO access from host failed due to
/// some error in PRING [GF100-]". **[EXT]** — envytools `docs/hw/bus/pbus.rst`
/// as quoted by PROPOSAL-kepler-fence-pull30 §"Decoding PBUS_INTR=0x0C" and
/// repeated in pull 31. The bench has observed the bit SET (s33's `0x0C`); the
/// NAME is the external part.
const PBUS_INTR_MMIO_RING_ERR: u32 = 1 << 2;
/// `PBUS_INTR` bit 3 — `MMIO_FAULT`, "MMIO access from host failed due to other
/// reasons [NV41-]". **[EXT]**, same source and same standing as bit 2.
const PBUS_INTR_MMIO_FAULT: u32 = 1 << 3;

/// The PIBUS (PRI ring) fault-reporting trio. **[METAL s33]** for existence —
/// all three read real `00000000` on this part at s33boot1
/// (`:: kepler: recon PIBUS_INTR_ADDR=00000000 :: VALUE=00000000 INTR=00000000`)
/// — and **[EXT]** for the names and addresses (envytools `docs/hw/mmio.rst`
/// plus `rnndb/bus/pibus.xml`, as quoted by PROPOSAL-kepler-fence-pull29:
/// "`INTR_ADDR` (`0x120120`), `INTR_VALUE` (`0x120124`), and `INTR`
/// (`0x120128`)"). `INTR_ADDR`/`INTR_VALUE` are REPORTING registers — they say
/// WHICH access faulted — and only `INTR` is treated as clearable below.
const PIBUS_INTR_ADDR: usize = 0x0012_0120;
const PIBUS_INTR_VALUE: usize = 0x0012_0124;
const PIBUS_INTR: usize = 0x0012_0128;

/// `PIBUS_MMIO_HUB_ENABLE1` (unicast). **[METAL s33]** — read `FFF9F4B0` on this
/// part, bit 4 (`CTXCTL` enable) already SET, which is the reading that REFUTED
/// the subunit-gating theory (`falcon_microcode_spec.md` §6 refutation 5).
/// Carried here read-only and as CONTEXT only: if the poison flips it, that is
/// a fact worth having; nothing in the verdict ladder reads it.
const PIBUS_MMIO_HUB_ENABLE1: usize = 0x0012_2104;

/// The baseline census, read BEFORE the deliberate poison. Absolute BAR0
/// addresses, read-only, and **not one of them is in the FECS unit** — the
/// `cpuctl`/`mailbox0` pair is read separately and named separately, because
/// mixing an in-unit read into this list is how a census fires the fault it is
/// supposed to precede.
const PRING_CENSUS: [(usize, &str, &str); 5] = [
    (PBUS_INTR, "pbus_intr", "METAL s33 (=0000000C, W1C'd) + EXT name"),
    (PIBUS_INTR_ADDR, "pibus_intr_addr", "METAL s33 (=00000000) + EXT name"),
    (PIBUS_INTR_VALUE, "pibus_intr_value", "METAL s33 (=00000000) + EXT name"),
    (PIBUS_INTR, "pibus_intr", "METAL s33 (=00000000) + EXT name"),
    (PIBUS_MMIO_HUB_ENABLE1, "pibus_hub_enable1", "METAL s33 (=FFF9F4B0, bit4 set)"),
];

/// Index of `pbus_intr` in [`PRING_CENSUS`], and of `pibus_intr`. Pinned by a
/// `const _`: the clear step addresses these two slots by number.
const PRING_PBUS_INTR: usize = 0;
const PRING_PIBUS_INTR: usize = 3;

/// The two registers the clear step may write, and NOTHING else may be added
/// here without its own metal reading. Both are W1C by the same external
/// source that names them, and both were read real on this part at s33.
///
/// `PIBUS_INTR_ADDR`/`PIBUS_INTR_VALUE` are deliberately ABSENT: they report
/// the faulting address and data, they are not documented as latches, and a
/// write-back to a reporting register asserts a semantics nothing here has.
const PRING_CLEARABLE: [(usize, &str, &str); 2] = [
    (
        PBUS_INTR,
        "pbus_intr",
        "METAL s33 — the W1C was PERFORMED on this part in the pull-29 boot ('PBUS_INTR=0000000C ... bits 2+3 latched; W1C'd') and again in pull 30's landed chain (0e26447e). Not an EXT-only write",
    ),
    (
        PIBUS_INTR,
        "pibus_intr",
        "METAL s33 for the register (read real 00000000) + EXT for W1C (envytools docs/hw/mmio.rst via PROPOSAL-pull29/30, whose landed chain carried exactly this conditional write-back). Written ONLY with bits read SET this boot, which is a no-op by construction if the register is not in fact W1C",
    ),
];

/// The PRING registers this rung READS and will NEVER write, each with the
/// reason — printed verbatim on the wire, the discipline KF27's [`SKIPPED_WRITES`]
/// and KF28's [`CTXBIND_SKIPPED_WRITES`] set: an absent line reads as a rung that
/// simply had nothing to write, and here that would hide a judgement.
///
/// Note what is NOT in this list: a `reason=uncited` row. The brief that ordered
/// this rung provided for one — if the PRING clear register were **[UNPINNED]**,
/// the clear would be a skipped write. It is not. Both registers in
/// [`PRING_CLEARABLE`] were read real ON THIS PART at s33boot1 and `PBUS_INTR`'s
/// W1C was PERFORMED there, so the clear rests on an observation of this silicon
/// and not on an external document alone — which is the line §0.1 draws when it
/// says EXT may name a register but may never be the sole basis for a write.
const PRING_NEVER_WRITTEN: [(&str, &str); 2] = [
    (
        "pring_clear_pibus_intr_addr",
        "not-a-latch — 0x120120 REPORTS the address of the faulting access (envytools rnndb/bus/pibus.xml via PROPOSAL-pull29, for a register read real 00000000 on this part at s33). Nothing names it write-1-to-clear, so a write-back here would assert a semantics this bench has not exercised",
    ),
    (
        "pring_clear_pibus_intr_value",
        "not-a-latch — 0x120124 REPORTS the data of the faulting access, same source and same standing as INTR_ADDR above",
    ),
];

const _: () = {
    // 1. THE POISON LAW, as an assertion rather than a promise: the BASELINE
    //    census may never contain the poison offset, or the rung fires its own
    //    experiment before it has taken the "before" reading and the whole boot
    //    says nothing.
    let mut i = 0;
    while i < PRING_CENSUS.len() {
        assert!(
            PRING_CENSUS[i].0 != FECS_WRCMD_CMD,
            "the KFUNWEDGE baseline census may never touch 0x409504 — it is the STIMULUS, not a census row"
        );
        // 2. And no census row is in the FECS unit at all. The unit is read by
        //    `fecs_cpuctl_mailbox` alone, which is what keeps the access ledger
        //    interpretable.
        assert!(
            PRING_CENSUS[i].0 < 0x0040_9000 || PRING_CENSUS[i].0 >= 0x0040_A000,
            "a PRING census row must lie outside the FECS unit"
        );
        i += 1;
    }
    assert!(PRING_CENSUS[PRING_PBUS_INTR].0 == PBUS_INTR, "pbus_intr index pinned");
    assert!(PRING_CENSUS[PRING_PIBUS_INTR].0 == PIBUS_INTR, "pibus_intr index pinned");
    // 3. Nothing clearable is in the FECS unit or is the poison offset. A W1C
    //    aimed into the unit would be an uncited write into the block §5.4 put
    //    under a standing ban.
    let mut j = 0;
    while j < PRING_CLEARABLE.len() {
        assert!(PRING_CLEARABLE[j].0 != FECS_WRCMD_CMD, "the clear may never write 0x409504");
        assert!(
            PRING_CLEARABLE[j].0 < 0x0040_0000,
            "the clear may never write inside PGRAPH or either falcon"
        );
        j += 1;
    }
    // 4. The two in-unit reads are the two §2 rows this rung claims, and the
    //    poison offset is neither of them.
    assert!(FECS_CPUCTL == 0x0040_9000 + 0x100, "cpuctl is the FECS unit base + 0x100");
    assert!(FECS_MAILBOX0 == 0x0040_9000 + 0x040, "mailbox0 is the FECS unit base + 0x040");
    assert!(FECS_CPUCTL != FECS_WRCMD_CMD && FECS_MAILBOX0 != FECS_WRCMD_CMD);
    // 5. KF27's probe window cannot reach the poison offset or the unit — the
    //    same assertion `mod ctxbind` makes, restated so an edit to either
    //    constant is caught here too.
    assert!(!candidate_admissible(0x0040_9000), "the FECS unit must be outside KF27's probe window");
};

// ===========================================================================
// The R19 gate — KF27's base ladder, evaluated read-only by this rung
// ===========================================================================

/// KF27's ladder, restated over KF27's OWN constants, exactly as `mod ctxbind`
/// restates it and for the same two reasons.
///
/// 1. `KEPLER-METAL-LOG.md`'s KF28 entry forbids flying KF27 with another rung
///    that acts in the same boot, and this rung's whole point is to act — so
///    `nvidia-kepler-kfunwedge` does not imply `nvidia-kepler-kfbind` and
///    `KfbindPre` may not exist at all on an armed boot.
/// 2. R19: the rung may not be scored at an address KF26 has been misreading
///    since s#6. That applies here even though this rung reads no PBDMA state,
///    because the register row it writes sits in §2's PBDMA ladder and a rung
///    that ran under an unresolved base would be filed beside ten verdicts that
///    were.
///
/// Returns `(class-name, usable)`. The class strings are KF27's verdict arms'
/// leading tokens verbatim, so `awk 'index($0,"DERIVED-WINS")'` finds all three
/// rungs' captures in one vocabulary.
fn kf27_base_class(bar0: usize) -> (&'static str, bool) {
    let br = Bracket::open(bar0);

    let mut derived = [0usize; MAX_DERIVED];
    let mut derived_n = 0usize;
    for i in 0..PTOP_ENTRIES {
        let v = unsafe { mmio_read(bar0, PTOP_DEVICE_INFO + i * 4) };
        match classify_fecs_word(v) {
            "POISON" | "ZERO" => continue,
            _ => {}
        }
        let cand = ptop_pri_candidate(v);
        if !candidate_admissible(cand) {
            continue;
        }
        if derived[..derived_n].contains(&cand) || derived_n == MAX_DERIVED {
            continue;
        }
        derived[derived_n] = cand;
        derived_n += 1;
    }

    let mut derived_answers = false;
    for &b in derived[..derived_n].iter() {
        if PbdmaSample::read(bar0, b).answers() {
            derived_answers = true;
        }
    }
    let mut legacy_answers = false;
    for i in 0..PBDMA_LEGACY_N {
        if PbdmaSample::read(bar0, PBDMA_LEGACY_BASE + i * PBDMA_LEGACY_STRIDE).answers() {
            legacy_answers = true;
        }
    }

    let (pre, post, held) = br.close(bar0);
    let class = if !held {
        "VOID-BRACKET"
    } else if derived_n == 0 {
        "LEGACY-ONLY"
    } else if derived_answers && !legacy_answers {
        "DERIVED-WINS"
    } else if legacy_answers && !derived_answers {
        "LEGACY-WINS"
    } else if derived_answers && legacy_answers {
        "BOTH-ANSWER"
    } else {
        "NEITHER-ANSWERS"
    };

    serial_println!(
        ":: KFUNWEDGE: base class={} derived_n={} legacy_answers={} ctl_pre={:08X} ctl_post={:08X} ctl={} — KF27's OWN ladder re-evaluated READ-ONLY on this boot over KF27's own PTOP address, extraction, window and probe offsets. NOT a KF27 flight: KF27's knob may be off and its lines absent, and the two rungs are FORBIDDEN to fly together ::",
        class, derived_n,
        if legacy_answers { "Y" } else { "n" },
        pre, post,
        if held { "held" } else { "MOVED — every dword above is uninterpretable" }
    );

    (class, class == "DERIVED-WINS" || class == "BOTH-ANSWER")
}

// ===========================================================================
// Instrument helpers
// ===========================================================================

/// The five PRING words, read in one pass at a named phase.
#[derive(Clone, Copy)]
struct PringSample {
    vals: [u32; PRING_CENSUS.len()],
}

impl PringSample {
    fn read(bar0: usize) -> Self {
        let mut vals = [0u32; PRING_CENSUS.len()];
        for (slot, (addr, _, _)) in vals.iter_mut().zip(PRING_CENSUS.iter()) {
            *slot = unsafe { mmio_read(bar0, *addr) };
        }
        Self { vals }
    }

    fn pbus_intr(&self) -> u32 {
        self.vals[PRING_PBUS_INTR]
    }

    /// Print every word with its address, its class and its citation, and — for
    /// `PBUS_INTR` alone — the two bits this tree has a name for, each tagged.
    fn print(&self, phase: &str) {
        for (v, (addr, name, cls)) in self.vals.iter().zip(PRING_CENSUS.iter()) {
            serial_println!(
                ":: KFUNWEDGE: pring {} {}={:08X} {} addr={:06X} cls=[{}] ::",
                phase, name, v, classify_fecs_word(*v), addr, cls
            );
        }
        let p = self.pbus_intr();
        serial_println!(
            ":: KFUNWEDGE: pring {} pbus_intr_bits raw={:08X} bit2_MMIO_RING_ERR={} [EXT envytools docs/hw/bus/pbus.rst via PROPOSAL-pull30] bit3_MMIO_FAULT={} [EXT same] other_bits={:08X} [UNTAGGED — this tree names no other PBUS_INTR bit, so a set bit here is PRINTED and NOT decoded] ::",
            phase, p,
            if p & PBUS_INTR_MMIO_RING_ERR != 0 { "SET" } else { "clr" },
            if p & PBUS_INTR_MMIO_FAULT != 0 { "SET" } else { "clr" },
            p & !(PBUS_INTR_MMIO_RING_ERR | PBUS_INTR_MMIO_FAULT)
        );
    }
}

/// The FECS unit's two proven rows, plus the out-of-unit control, in one
/// reading. Returns `(cpuctl, mailbox0, pmc_boot_0)`.
///
/// `fecs_read`, never a bare `read_volatile`: it is the accessor that keeps the
/// campaign's FECS access ledger (`FECS_ACCESS_COUNT`, `FECS_FIRST_OFFSET`, the
/// `0x409504` touch flags) honest, and the `fecs-ledger` line at the end of the
/// boot is one of this rung's deliverables.
fn fecs_and_control(bar0: usize) -> (u32, u32, u32) {
    let cpuctl = crate::drivers::gpu::kepler::fecs_read(bar0, FECS_CPUCTL);
    let mailbox0 = crate::drivers::gpu::kepler::fecs_read(bar0, FECS_MAILBOX0);
    let ctl = unsafe { mmio_read(bar0, regs::NV_PMC_BOOT_0) };
    (cpuctl, mailbox0, ctl)
}

// ===========================================================================
// The rung
// ===========================================================================

/// Runs from `kepler::init` as the LAST kepler statement before the terminal
/// poke, and that placement is a CONTRACT in the strongest form this file has.
///
/// `falcon_microcode_spec.md` §5.4: *put unproven offsets last, after every
/// proven read has completed.* This rung's stimulus is not merely unproven, it
/// is the offset the law is NAMED for — so every FECS reading of the boot,
/// every display leg, every FIFO verdict and both halves of KF27/KF28 are
/// already complete and already printed when it fires. Nothing after it in the
/// kepler leg reads the unit except the terminal poke, which is a write with no
/// readback.
///
/// **Stimulus:** the KF27 base sweep's reads, 5 PRING reads ×3 phases, 3 FECS
/// reads ×3 phases (`cpuctl`, `mailbox0`, and the out-of-unit control), ONE
/// read of `0x409504`, and up to 2 W1C write-backs of bits observed SET.
pub fn kfunwedge(bar0: usize) {
    serial_println!(
        ":: KFUNWEDGE: begin knob=UNAOS_KEPLER_KFUNWEDGE rung=KF29 subject=\"the un-wedge experiment KF21 opened and ten sittings never ran: fire the 0x409504 poison ON PURPOSE, then observe and clear the PRI ring and re-read cpuctl in the SAME boot\" restored=IMPOSSIBLE-a-poison-is-not-restorable sacrificial=YES — ⚠ THIS BOOT IS SACRIFICIAL. Fly it LAST in a sitting and ALONE: never with BEAMX86, KDHEAD, UNAOS_KEPLER_KFBIND (KF27) or UNAOS_KEPLER_KFCTXBIND (KF28). The Kepler drives the panel on this machine, so the operator may LOSE THE DISPLAY until a POWER CYCLE; the serial capture is the deliverable ::"
    );

    // ---- 0. The R19 gate ---------------------------------------------------
    let (base_class, usable) = kf27_base_class(bar0);
    if !usable {
        serial_println!(
            ":: KFUNWEDGE: skipped reason=kf27-base-unresolved class={} — R19: this rung files its verdict in register 2's PBDMA ladder, beside ten eliminations read at an address KF26 has never derived, and it will not add an eleventh row under an unresolved base. NOTHING WAS MEASURED and, more importantly, NOTHING WAS POISONED: 0x409504 was not read, the unit is intact, and this boot is NOT the sacrificial one ::",
            base_class
        );
        serial_println!(
            ":: KFUNWEDGE: end rung=KF29 skipped=kf27-base-unresolved base={} poisoned=not-attempted clear=not-attempted -> NOT-RUN — one rollup line even when the gate refused, so a silent rung is distinguishable from a quiet pass ::",
            base_class
        );
        return;
    }

    let br = Bracket::open(bar0);

    // ---- 1. The baseline, read-only, BEFORE anything is poked --------------
    let pre_pring = PringSample::read(bar0);
    pre_pring.print("pre");
    let (pre_cpuctl, pre_mb0, pre_ctl) = fecs_and_control(bar0);

    let (acc, first, r_touched, r_idx, w_touched, w_idx) =
        crate::drivers::gpu::kepler::fecs_poison_ledger();
    serial_println!(
        ":: KFUNWEDGE: pre pring=0x{:08X} cpuctl=0x{:08X} {} mailbox0=0x{:08X} {} ctl=0x{:08X} {} sentinel_504_read={} idx={} sentinel_504_write={} idx={} fecs_accesses={} first_offset={:08X} — pring= is PBUS_INTR, the one PRING word this bench has ever seen carry a nonzero value (0x0000000C at s33). sentinel_504_read MUST read n here: if it reads Y something above this rung already fired the poison and every 'before' value on this line is an AFTER value ::",
        pre_pring.pbus_intr(),
        pre_cpuctl, classify_fecs_word(pre_cpuctl),
        pre_mb0, classify_fecs_word(pre_mb0),
        pre_ctl, classify_fecs_word(pre_ctl),
        if r_touched { "Y" } else { "n" }, r_idx,
        if w_touched { "Y" } else { "n" }, w_idx,
        acc, first
    );

    // ---- 2. THE DELIBERATE POISON. Exactly one read, and this is it --------
    //
    // The rung it cites: §2 KF21 `terminal-poke 0x409504` — "the poison
    // register is writable without consequence to the boot. Reading it first is
    // what poisons; writing it last is harmless." s31 discovered the read-side
    // law, s32 confirmed it with its own control frame, s34 convicted this
    // offset by elimination once the other six CTXCTL offsets all read clean.
    // KF20 is the row that carries the ⚠ silicon law itself.
    serial_println!(
        ":: KFUNWEDGE: poke-pre about-to-read=0x409504 rung-cited=KF21/KF20 law=falcon_microcode_spec.md-5.4 cls=[METAL s31/s32/s34] — printed BEFORE the access, the §10 discipline, so the capture proves the ordering even if the unit takes the serial path with it ::"
    );
    let poke = crate::drivers::gpu::kepler::fecs_read(bar0, FECS_WRCMD_CMD);
    let poke_cls = classify_fecs_word(poke);
    let poisoned = poke_cls == "POISON";
    serial_println!(
        ":: KFUNWEDGE: poke read=0x409504 value={:08X} {} poisoned={} family={:04X} — the BAD0/BADF family is the nonexistent-PRI-register signature (spec 5.4, s25); BADF1000 is what s31/s32/s34 read at this exact offset. poisoned=no would mean the read did NOT fault on this boot, which is a bigger finding than the un-wedge and is scored as NOT-POISONED below ::",
        poke, poke_cls,
        if poisoned { "yes" } else { "no" },
        (poke >> 16) as u16
    );

    // ---- 3. OBSERVE. Did the fault reach the ring, and how far did it spread?
    let obs_pring = PringSample::read(bar0);
    obs_pring.print("post-poke");
    let (obs_cpuctl, obs_mb0, obs_ctl) = fecs_and_control(bar0);
    let unit_poisoned =
        classify_fecs_word(obs_cpuctl) == "POISON" || classify_fecs_word(obs_mb0) == "POISON";
    let control_poisoned = classify_fecs_word(obs_ctl) == "POISON";
    serial_println!(
        ":: KFUNWEDGE: observe pring=0x{:08X} pring_moved={} cpuctl=0x{:08X} {} mailbox0=0x{:08X} {} ctl=0x{:08X} {} unit_poisoned={} control_poisoned={} — cpuctl is the SAME register the pre line read, which is s32's control-frame shape exactly. ctl is NV_PMC_BOOT_0, OUTSIDE the FECS unit: a real chip ID beside a poisoned cpuctl BOUNDS the damage to the unit (s31 inferred that from PFIFO; this reads it), and a poisoned ctl says the whole BAR0 path is down and every line after this one is worthless ::",
        obs_pring.pbus_intr(),
        if obs_pring.pbus_intr() == pre_pring.pbus_intr() { "n" } else { "Y" },
        obs_cpuctl, classify_fecs_word(obs_cpuctl),
        obs_mb0, classify_fecs_word(obs_mb0),
        obs_ctl, classify_fecs_word(obs_ctl),
        if unit_poisoned { "Y" } else { "n" },
        if control_poisoned { "Y" } else { "n" }
    );

    // ---- 4. CLEAR. W1C, and ONLY bits this boot read as SET -----------------
    //
    // The write discipline, stated once: writing back the exact value just READ
    // is the least-assumptive write available. In a W1C register it clears
    // precisely the latched bits and nothing else; in a plain RW register it is
    // a no-op by construction. So the write asserts no field layout, no command
    // encoding, and no bit meaning beyond "these bits were set a microsecond
    // ago" — which is an observation of this boot, not a citation.
    //
    // A register reading ZERO is NOT written. There is nothing latched to clear,
    // and a zero write into a register whose semantics this bench has not
    // exercised would be exactly the uncited write pull 28's ban was about.
    for (name, reason) in PRING_NEVER_WRITTEN.iter() {
        serial_println!(":: KFUNWEDGE: skipped write={} reason={} ::", name, reason);
    }

    let mut cleared = 0usize;
    let mut skipped = 0usize;
    for (addr, name, cls) in PRING_CLEARABLE.iter() {
        let latched = match *addr {
            a if a == PBUS_INTR => obs_pring.vals[PRING_PBUS_INTR],
            _ => obs_pring.vals[PRING_PIBUS_INTR],
        };
        if latched == 0 {
            skipped += 1;
            serial_println!(
                ":: KFUNWEDGE: skipped write=pring_clear_{} reason=nothing-latched value={:08X} addr={:06X} cls=[{}] — the register answers and holds no set bit, so there is no W1C to perform. Writing 0 would assert a meaning for a field this bench has not exercised ::",
                name, latched, addr, cls
            );
            continue;
        }
        if classify_fecs_word(latched) == "POISON" || classify_fecs_word(latched) == "ABSENT" {
            skipped += 1;
            serial_println!(
                ":: KFUNWEDGE: skipped write=pring_clear_{} reason=readback-not-a-value value={:08X} {} addr={:06X} — a W1C of a POISON/ABSENT readback would write a fault signature back into the register ::",
                name, latched, classify_fecs_word(latched), addr
            );
            continue;
        }
        serial_println!(
            ":: KFUNWEDGE: clear write=pring_clear_{} addr={:06X} w1c={:08X} cls=[{}] — writing back EXACTLY the bits read as set one line above: a clear in a W1C register, a no-op by construction in a plain RW one ::",
            name, addr, latched, cls
        );
        unsafe { crate::drivers::gpu::kepler::mmio_write(bar0, *addr, latched) };
        cleared += 1;
    }

    // ---- 5. RE-READ, and the verdict ---------------------------------------
    let post_pring = PringSample::read(bar0);
    post_pring.print("post-clear");
    let (post_cpuctl, post_mb0, post_ctl) = fecs_and_control(bar0);
    let (ctl_pre, ctl_post, held) = br.close(bar0);

    let unit_real_now = classify_fecs_word(post_cpuctl) != "POISON"
        && classify_fecs_word(post_mb0) != "POISON";

    let verdict = if !held {
        "VOID-BRACKET — NV_PMC_BOOT_0 moved across the rung, so nothing above is interpretable and no statement is made about the poison, the ring or the clear"
    } else if control_poisoned {
        "VOID-CONTROL — the out-of-unit control (NV_PMC_BOOT_0) read POISON at the observe step. The BAR0 path itself is down, not the FECS unit, and this rung cannot tell a wedged ring from a dead link. NOT a statement about the un-wedge"
    } else if !poisoned && !unit_poisoned {
        "NOT-POISONED — the deliberate read of 0x409504 returned a non-POISON word and neither cpuctl nor mailbox0 faulted afterwards. The poison law did NOT fire on this boot, so the un-wedge was again not exercised (s33boot1's exact miss) — and that is a LOUDER result than an un-wedge would have been: s31/s32/s34 recorded this offset faulting on first access three times, so a clean read here names a CONDITION those three boots did not share. NOT 'ruled out' (R19): the conditions are the ones printed on this rung's own lines"
    } else if poisoned && !unit_poisoned {
        "POISON-CONFINED — the read of 0x409504 returned a fault word, but neither cpuctl nor mailbox0 faulted after it. The offset answers POISON for ITSELF without wedging the unit, which CONTRADICTS s31/s32's spread and is a per-offset finding, not an un-wedge. The clear's outcome below says nothing either way: there was nothing wedged to recover"
    } else if unit_real_now {
        "UNWEDGED — the unit was POISONED at the observe step and reads REAL again after the clear. A PRING W1C recovers a GK107 FECS unit inside the boot, the question KF21 opened in July is answered, and every future probe of an unproven 0x409xxx offset can be made survivable"
    } else {
        "STILL-POISONED"
    };

    serial_println!(
        ":: KFUNWEDGE: verdict base={} poke={:08X} {} poisoned={} unit_poisoned_at_observe={} pring {:08X}->{:08X}->{:08X} cpuctl {:08X}->{:08X}->{:08X} mailbox0 {:08X}->{:08X}->{:08X} ctl {:08X}->{:08X}->{:08X} clear={{written={} skipped={}}} ctl_pre={:08X} ctl_post={:08X} ctl={} restored=IMPOSSIBLE -> {} under {{clear={}, base={}, control={}, sacrificial-boot=YES, rung-ordering=last-kepler-statement-before-the-terminal-poke}} ::",
        base_class,
        poke, poke_cls, if poisoned { "yes" } else { "no" },
        if unit_poisoned { "Y" } else { "n" },
        pre_pring.pbus_intr(), obs_pring.pbus_intr(), post_pring.pbus_intr(),
        pre_cpuctl, obs_cpuctl, post_cpuctl,
        pre_mb0, obs_mb0, post_mb0,
        pre_ctl, obs_ctl, post_ctl,
        cleared, skipped,
        ctl_pre, ctl_post,
        if held { "held" } else { "MOVED" },
        verdict,
        if cleared > 0 { "written" } else { "skipped" },
        base_class,
        if control_poisoned { "POISONED" } else { "real" }
    );

    serial_println!(
        ":: KFUNWEDGE: end rung=KF29 poisoned={} clear={} unwedged={} writes={} restored=IMPOSSIBLE — a poison is not restorable: there is no write that un-reads a read, and the only bound on this boot's damage is the POWER CYCLE. ⚠ THE TERMINAL POKE BELOW IS NO LONGER THE FIRST ACCESS TO 0x409504 ON THIS BOOT, so spec 10's datum for it is VOID here; the fecs-ledger line states the ordering (504_read_idx < 504_write_idx) rather than leaving it to be assumed ::",
        if poisoned { "yes" } else { "no" },
        if cleared > 0 { "written" } else { "skipped" },
        if held && !control_poisoned && poisoned && unit_poisoned && unit_real_now { "YES" } else { "no" },
        cleared
    );
}
} // mod unwedge
