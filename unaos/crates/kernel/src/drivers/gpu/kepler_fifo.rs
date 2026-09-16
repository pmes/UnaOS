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
