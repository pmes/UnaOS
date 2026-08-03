// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// BCM2711 EMMC2 / SDHCI microSD driver (M6g) — the block backend that lets the bare-metal Pi 4 load a
// program off the very card it booted from. Deliberately minimal, matching the arc scope: PIO only,
// single-block CMD17 reads, fully polled (IRPT_EN = 0), no DMA, and NO writes (the seam in
// `drivers::block` refuses a write on this backend). Every wait is bounded by a CNTPCT deadline — the
// mailbox driver's discipline (arch/aarch64/mailbox.rs) — so a missing/silent controller fails the probe
// and logs one clean line instead of hanging boot.
//
// Dual-base probe. QEMU's `raspi4b` attaches an `if=sd` card to the LEGACY Arasan SDHCI @0xFE30_0000;
// the EMMC2 controller @0xFE34_0000 has no card. On real Pi 4 silicon it is the reverse: the microSD
// slot routes to EMMC2 and the legacy Arasan carries the SDIO WiFi. So we probe **EMMC2 first, then fall
// back to the legacy base** — one driver, two candidate bases, and the metal path is the first tried.
// QEMU therefore always exercises the fallback (legacy) leg; the EMMC2 success leg runs on silicon only.
//
// Register map: the BCM2835-style names are 32-bit views of the standard SDHCI register block, which is
// exactly what QEMU's generic-sdhci and the real Arasan/EMMC2 both serve — so the same accessors drive
// both. Both bases sit in the peripheral GiB already mapped Device/XN by `boot::build_l1` (L1[3] @
// 0xC000_0000), so no MMU change is needed.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use spin::Mutex;

use crate::drivers::block::{BlockDeviceInfo, BlockError};

// --- Candidate controller bases (Pi 4 low-peripheral mode, peripheral base 0xFE00_0000). ---
const EMMC2_BASE: usize = 0xFE34_0000; // EMMC2 — the real microSD slot on metal; no card in QEMU
const LEGACY_BASE: usize = 0xFE30_0000; // legacy Arasan SDHCI — QEMU's `if=sd` card; SDIO WiFi on metal

// RPi firmware GET_CLOCK_RATE ids, used only if a base's own CAPABILITIES base-clock field reads zero.
const CLK_ID_EMMC2: u32 = 0xC; // EMMC2 clock (0xFE34 base)
const CLK_ID_EMMC: u32 = 0x1; // EMMC clock (0xFE30 legacy base)

// --- SDHCI register offsets (32-bit views). ---
const BLKSIZECNT: usize = 0x04; // [15:0] block size, [31:16] block count
const ARG1: usize = 0x08;
const CMDTM: usize = 0x0C; // command + transfer-mode; writing this ISSUES the command
const RESP0: usize = 0x10;
const RESP1: usize = 0x14;
const RESP2: usize = 0x18;
const RESP3: usize = 0x1C;
const DATA: usize = 0x20; // PIO FIFO, 32-bit LE
const STATUS: usize = 0x24;
const CONTROL0: usize = 0x28;
const CONTROL1: usize = 0x2C;
const INTERRUPT: usize = 0x30; // W1C
const IRPT_MASK: usize = 0x34; // status-ENABLE (bits latch into INTERRUPT only if set here)
const IRPT_EN: usize = 0x38; // signal-enable (kept 0 — polled)
const CAPABILITIES: usize = 0x40;

// --- STATUS (0x24) bits. ---
const ST_CMD_INHIBIT: u32 = 1 << 0;
const ST_DAT_INHIBIT: u32 = 1 << 1;

// --- CONTROL1 (0x2C) bits. ---
const C1_CLK_INTLEN: u32 = 1 << 0;
const C1_CLK_STABLE: u32 = 1 << 1;
const C1_CLK_EN: u32 = 1 << 2;
const C1_SRST_HC: u32 = 1 << 24;

// --- INTERRUPT (0x30) bits (W1C). ---
const INT_CMD_DONE: u32 = 1 << 0;
const INT_DATA_DONE: u32 = 1 << 1;
const INT_WRITE_RDY: u32 = 1 << 4; // Buffer Write Ready (U9 write path); the READ path uses bit 5 below
const INT_READ_RDY: u32 = 1 << 5;
const INT_ERR: u32 = 1 << 15; // summary of any error interrupt (bits 16+)
/// Any error: the error-summary bit OR any of the error-status bits [31:16].
const INT_ERR_ANY: u32 = INT_ERR | 0xFFFF_0000;

// --- R1 card-status error bits (SD Physical Layer §4.10.1). The R1 word rides RESP0 after any R1
// command; these are the card-REPORTED failure bits — OUT_OF_RANGE(31), ADDRESS_ERROR(30),
// BLOCK_LEN_ERROR(29), ERASE_SEQ_ERROR(28), ERASE_PARAM(27), WP_VIOLATION(26), CARD_IS_LOCKED(25),
// LOCK_UNLOCK_FAILED(24), COM_CRC_ERROR(23), ILLEGAL_COMMAND(22), CARD_ECC_FAILED(21), CC_ERROR(20),
// ERROR(19), CSD_OVERWRITE(16), WP_ERASE_SKIP(15), AKE_SEQ_ERROR(3). The controller's INT error bits
// only cover the LINK (CRC/timeout/index); a card can complete the command cleanly at the link layer
// while flagging e.g. WP_VIOLATION here — without this check that failure is silently swallowed. ---
const R1_ERROR_MASK: u32 = 0xFFF9_8008;

// --- CMDTM (0x0C) field builders. ---
const CMD_RESP_NONE: u32 = 0b00 << 16;
const CMD_RESP_136: u32 = 0b01 << 16;
const CMD_RESP_48: u32 = 0b10 << 16;
const CMD_RESP_48_BUSY: u32 = 0b11 << 16;
const CMD_CRCCHK: u32 = 1 << 19;
const CMD_IXCHK: u32 = 1 << 20;
const CMD_ISDATA: u32 = 1 << 21;
const CMD_DAT_DIR_READ: u32 = 1 << 4; // 1 = card -> host
const CMD_DAT_DIR_WRITE: u32 = 0 << 4; // 0 = host -> card (U9; a no-op OR — the point is to OMIT DAT_DIR_READ)
#[inline]
const fn cmd(index: u32) -> u32 {
    index << 24
}

// --- Bounded-wait budgets (ms). Generous vs the microseconds QEMU/real cards take; the point is to fail
// a dead/absent controller cleanly rather than hang boot. ---
const CMD_TIMEOUT_MS: u64 = 100; // one command's completion
const ACMD41_TIMEOUT_MS: u64 = 1000; // the power-up (busy) polling loop
const RESET_TIMEOUT_MS: u64 = 100;
const CLK_STABLE_TIMEOUT_MS: u64 = 100;
const DATA_TIMEOUT_MS: u64 = 200;
/// Post-write programming-busy bound: the SD spec caps a single-block write's busy window at 250 ms
/// (§4.6.2.2 write timeout); double it for margin. Distinct from CMD_TIMEOUT_MS (100 ms) — waiting for
/// programming under the plain command timeout would misclassify a slow-but-successful write as an error.
const PROG_BUSY_TIMEOUT_MS: u64 = 500;

/// WEDGE-10 (F2): how long a MASKED claimant may spin re-attempting [`claim`] before giving up with
/// `BlockError::Busy`. Derived from this driver's own worst legitimate hold, not guessed: the longest
/// transfer is the CMD24 write ladder, whose deadlines sum to ~1300 ms — `CMD_TIMEOUT_MS` twice for the
/// CMD24 issue (DAT/CMD-inhibit + CMD_DONE), `DATA_TIMEOUT_MS` twice (WRITE_RDY + DATA_DONE),
/// `PROG_BUSY_TIMEOUT_MS` for the programming-busy window, and `CMD_TIMEOUT_MS` twice more for the CMD13
/// verdict. The budget is 2× that, so a masked claimant outlasts one entire worst-case transfer by a
/// factor of two before it concludes the holder is not coming back. See [`claim_for_io`] for why a
/// wall-clock-bounded masked wait is a stall rather than the F2 deadlock.
const MASKED_CLAIM_BUDGET_MS: u64 = 2 * (2 * CMD_TIMEOUT_MS + 2 * DATA_TIMEOUT_MS + PROG_BUSY_TIMEOUT_MS + 2 * CMD_TIMEOUT_MS);

/// The identified card: which base won the probe, its addressing mode, and its capacity. Loaned out by
/// value for the duration of one sector transfer — see [`CARD`] for why it is a loan and not a held lock.
struct SdCard {
    base: usize,
    /// ccs (from ACMD41 bit30): true = SDHC/SDXC block addressing, false = SDSC byte addressing.
    block_addressing: bool,
    num_blocks: u64,
    csd_version: u8, // 1 or 2, for the identified line
    /// CMD13 SEND_STATUS argument (RCA already shifted into [31:16]), captured at CMD3.
    rca_arg: u32,
}
/// WEDGE-10 (F2) — the identified card lives behind a CLAIM/LOAN model, and this mutex is PRIVATE.
///
/// F2 is the last of the F1–F4 masked-spinner family, and its instance is F3's defect one layer down.
/// `CARD` used to be locked at the TOP of [`read_block_512`] / [`write_block_512`] and held straight
/// across the entire polled sector transfer. Those transfers are bounded — but by CNTPCT deadlines that
/// sum to ~600 ms on a read (`CMD`/`DAT` inhibit 100 + `CMD_DONE` 100 + `READ_RDY` 200 + `DATA_DONE`
/// 200) and ~1.3 s on a write (the same ladder plus `PROG_BUSY` 500 and a CMD13 round trip) — against a
/// 12 ms scheduler quantum. The holders are ordinary PREEMPTIBLE tasks: an EL0 read walking a cluster
/// chain, the `/fs/` unafs sector device, the shell's raw block verbs. Tasks never migrate and pinned
/// tasks are never stolen, so once a holder was preempted mid-hold it never ran again while a masked
/// acquirer (EL0 `SYS_WRITE` → `fat.rs` `without_interrupts` FAT/dir RMW → `drivers/block.rs` → here)
/// spun on this lock on that same core: that core could take no timer IRQ, the holder was never
/// re-dispatched, and the core died silently — no panic, and with `FAT_MUTATION` still held by the
/// spinner, the filesystem died with it. There is no ABBA cycle here either; lock ordering fixes none
/// of it.
///
/// F2 differs from F3 in RATE, not in kind, and that is why it was last rather than optional: a healthy
/// card's hold is microseconds, so the preempt window is a small fraction of each sector op instead of
/// F3's near-certainty — but FAT traffic runs thousands of sector ops per boot, and a slow or failing
/// card walks the bounded ladder out into the hundreds of milliseconds, where the window is the whole
/// hold. Lowest trigger rate in the family; identical silent core death when it triggers.
///
/// The fix is WEDGE-8's, not WEDGE-7's. F1's fix masked the critical section, affordable only for
/// micro-bounded work; a ~1.3 s worst-case sector ladder can no more be masked than F3's 8.3 s BOT pump
/// (masking a core for that long is the bug in another coat). So the discipline goes on the LOCK, not
/// the WORK:
///
///   * the mutex is held only inside [`claim`] / [`SdLoan::drop`] / [`install`], each a masked O(1)
///     take/put (the WEDGE-7 guard order: mask taken BEFORE the acquire, lock released BEFORE the
///     unmask). No masked spinner can wait more than a few dozen cycles on it, and no holder of it can
///     ever be preempted mid-hold.
///   * the CARD ITSELF is loaned out by value to exactly one transfer at a time, which runs the polled
///     CMD17/CMD24 ladder with NO lock held. A contender is told [`SdClaimError::Busy`] immediately
///     rather than spinning, and handles it honestly: [`claim_for_io`] waits only when unmasked, the
///     block layer surfaces `BlockError::Busy`, `fat.rs` retries the whole RMW OUTSIDE its masked span,
///     and EL0 sees `-EAGAIN`.
///
/// The whole claim/loan surface is module-private — tighter than F3's, which had to be `pub` because
/// the xHCI controller is driven from many files. Every SD transfer goes through this file's two entry
/// points, so nothing outside needs a loan. The invariant, checkable by grep: `CARD.lock()` appears
/// ONLY in `claim`/`Drop`/`install` in this file, and the compiler enforces the static's privacy.
static CARD: Mutex<Option<SdCard>> = Mutex::new(None);

/// WEDGE-10: true while the card is loaned out via [`claim`]. Written only inside the masked mutex
/// hold, so a `None` in the mutex disambiguates cleanly: loaned (`Busy`) vs never identified
/// (`NotReady`).
static CARD_LOANED: AtomicBool = AtomicBool::new(false);

/// WEDGE-10: the identified card's block count, published once by [`install`] and immutable after — so
/// it is readable with NO lock and NO loan. Load-bearing twice over. First, [`card_num_blocks`] is
/// called from inside `fs::unafs::with_unafs`'s masked span (via `SdSectorDevice::open`), where taking
/// `CARD` was itself an instance of this arc's defect. Second, under the loan model a lock-based read
/// would see `None` whenever a transfer happened to be in flight and silently report "no SD card" —
/// reverting the PI-FS-2 geometry fix to the shared `BLOCK_DEVICE` global for that call. 0 = no card
/// identified yet (`try_init` rejects a zero-block card, so 0 is an unambiguous sentinel).
static CARD_BLOCKS: AtomicU64 = AtomicU64::new(0);

/// PI-FS-2: the identified microSD card's block count, or `None` until [`probe`] succeeds.
///
/// Read-only geometry source for the unafs mount. The SD-backed volume must be *sized* by the SD
/// itself, never by whatever the shared `block::BLOCK_DEVICE` global happens to hold: a USB
/// mass-storage enumeration overwrites that global with the stick's geometry (see
/// `drivers::xhci` storage bring-up), so a mount that read its bound from the global would guard
/// the SD's own reads against a foreign device's block count. Since `read_block_512` already
/// serves SD data whenever the SD backend is active, its size guard must come from the same card
/// — this accessor is that source.
///
/// WEDGE-10 (F2): served from the immutable [`CARD_BLOCKS`] publication, so this is lock-free and
/// loan-independent — it neither blocks on nor is perturbed by an in-flight transfer.
pub fn card_num_blocks() -> Option<u64> {
    match CARD_BLOCKS.load(Ordering::Acquire) {
        0 => None,
        n => Some(n),
    }
}

/// WEDGE-10: why [`claim`] returned no card.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SdClaimError {
    /// No card was ever identified (both bases failed the probe, or `probe` has not run yet).
    NotReady,
    /// Another context holds the loan right now — a sector transfer is in flight. The claim did NOT
    /// wait: waiting is the caller's decision, and a masked caller must not.
    Busy,
}

/// WEDGE-10: an exclusive loan of the identified card, returned by [`claim`]. Derefs to [`SdCard`];
/// dropping it returns the card to the shared slot (masked O(1) put, panic-safe by RAII — the early
/// `return Err(..)` paths all over the transfer ladders rely on exactly that).
struct SdLoan(Option<SdCard>);

impl core::ops::Deref for SdLoan {
    type Target = SdCard;
    #[inline]
    fn deref(&self) -> &SdCard {
        self.0.as_ref().expect("SdLoan invariant: Some until drop")
    }
}

impl Drop for SdLoan {
    fn drop(&mut self) {
        if let Some(c) = self.0.take() {
            // WEDGE-10: masked micro-hold; the field order of these locals IS the fix in miniature —
            // the guard (lock) drops before `_mask` restores DAIF, so we never run unmasked holding it.
            let _mask = crate::arch::IrqMask::new();
            let mut guard = CARD.lock();
            *guard = Some(c);
            CARD_LOANED.store(false, Ordering::Release);
        }
    }
}

/// WEDGE-10: claim exclusive use of the card. O(1), never waits — the internal mutex hold is a masked
/// take (a preempted holder of it is impossible, so any spin on it is bounded by construction), and a
/// card already loaned out returns [`SdClaimError::Busy`] instead of blocking. Callers that can afford
/// to wait do so OUTSIDE this call, unmasked, with their own bounded policy ([`claim_for_io`]).
fn claim() -> Result<SdLoan, SdClaimError> {
    let _mask = crate::arch::IrqMask::new();
    let mut guard = CARD.lock();
    match guard.take() {
        Some(c) => {
            CARD_LOANED.store(true, Ordering::Release);
            Ok(SdLoan(Some(c)))
        }
        None => Err(if CARD_LOANED.load(Ordering::Acquire) {
            SdClaimError::Busy
        } else {
            SdClaimError::NotReady
        }),
    }
}

/// WEDGE-10: install the freshly identified card into the shared slot (BSP probe, once). Publishes the
/// immutable geometry FIRST so no reader can observe a claimable card whose block count is still 0,
/// then takes the same masked micro-hold as [`claim`].
fn install(card: SdCard) {
    CARD_BLOCKS.store(card.num_blocks, Ordering::Release);
    let _mask = crate::arch::IrqMask::new();
    let mut guard = CARD.lock();
    *guard = Some(card);
    CARD_LOANED.store(false, Ordering::Release);
}

/// WEDGE-10 (F2): claim the card for one sector transfer — the emmc2 twin of
/// `drivers::block::claim_xhci_for_io`. It lives HERE rather than beside the block layer's SD arms
/// because [`read_block_512`] / [`write_block_512`] are also called directly (the INSTALL-PI target in
/// `install::pi` bypasses `drivers::block` entirely), so the policy has to sit UNDER every entry point
/// rather than beside one of them.
///
/// Masked callers — the FAT/dir RMW spans under `fat.rs`'s `without_interrupts`, and
/// `fs::unafs::with_unafs` — get a CNTPCT-BOUNDED re-claim spin, then `BlockError::Busy`. They may NOT
/// `hlt` (a WFI under masked IRQs is not this policy's business) and they may NOT wait on the mutex; they
/// re-attempt the O(1) [`claim`] with `spin_loop` hints until [`MASKED_CLAIM_BUDGET_MS`] of wall clock
/// has passed.
///
/// WHY A MASKED WAIT IS SOUND HERE, AND IS NOT F3 AGAIN. The defect this arc closes was an UNBOUNDED
/// masked spin on a *lock* whose same-core preempted holder could never run again — the wait could only
/// end if the holder ran, and the holder could only run if the waiter stopped. This wait ends
/// unconditionally on wall clock, whether or not the holder ever runs, so no execution can be
/// indefinitely postponed: it is a bounded STALL, the same accepted cost class as the masked WFI stall
/// already documented on `fat.rs::with_fat_lock`. The two contention cases separate cleanly:
///
///   * CROSS-CORE (the common case, and the one the QEMU gate reproduces) — the loan holder runs on its
///     own core and every wait it performs is itself CNTPCT-bounded, so it returns the card well inside
///     this budget and the masked claimant proceeds. No stall in practice: a healthy hold is µs.
///   * SAME-CORE PREEMPTED HOLDER (the F2 corner) — the holder cannot run while we spin, so the budget
///     simply expires and we return `Busy`. Bounded stall, then an honest refusal that `fat.rs` retries
///     outside its mask. Never a dead core, which is the whole point.
///
/// FAIRNESS, deliberately inverted: the masked spinner polls far faster than the unmasked `hlt` waiters
/// below, so it wins the next free loan. That is intentional — an instant-`Busy` masked policy is
/// STARVED by construction on this backend, because emmc2's healthy hold is microseconds while every
/// unmasked competitor waits (and therefore wins) — which cost the U11 reaper its `free_chain` and
/// orphaned a cluster chain outright. The masked context is the one that cannot afford to lose 64 races.
///
/// Unmasked callers keep the old effectively-blocking semantics, honestly bounded: retry the claim with
/// a `hlt` between attempts (each wakes on the next IRQ, letting the scheduler run the loan holder) up
/// to `hw_wait_budget()` wall-clock — ~2.8 s on the Pi, comfortably past the ~1.3 s worst-case write
/// ladder, so a healthy card never surfaces `Busy` to an unmasked caller. A card wedged in a failing
/// transfer surfaces `Busy` instead of hanging its caller forever.
fn claim_for_io() -> Result<SdLoan, BlockError> {
    match claim() {
        Ok(l) => return Ok(l),
        Err(SdClaimError::NotReady) => return Err(BlockError::NotReady),
        Err(SdClaimError::Busy) => {}
    }
    if crate::arch::irqs_masked() {
        // Masked: spin on the O(1) claim under a CNTPCT deadline — never `hlt` (we are masked), never
        // the mutex. `deadline_ms`/`expired` are this driver's own bounded-wait discipline.
        let dl = deadline_ms(MASKED_CLAIM_BUDGET_MS);
        loop {
            core::hint::spin_loop();
            match claim() {
                Ok(l) => return Ok(l),
                Err(SdClaimError::NotReady) => return Err(BlockError::NotReady),
                Err(SdClaimError::Busy) => {}
            }
            if expired(dl) {
                return Err(BlockError::Busy);
            }
        }
    }
    let start = crate::arch::now_cycles();
    let budget = crate::arch::hw_wait_budget();
    loop {
        crate::hlt();
        match claim() {
            Ok(l) => return Ok(l),
            Err(SdClaimError::NotReady) => return Err(BlockError::NotReady),
            Err(SdClaimError::Busy) => {}
        }
        if crate::arch::now_cycles().wrapping_sub(start) >= budget {
            return Err(BlockError::Busy);
        }
    }
}

#[inline]
fn read32(base: usize, off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((base + off) as *const u32) }
}
#[inline]
fn write32(base: usize, off: usize, val: u32) {
    unsafe { core::ptr::write_volatile((base + off) as *mut u32, val) }
}

/// A CNTPCT deadline `ms` milliseconds from now (the free-running counter is monotonic and won't wrap in
/// any boot window, so a plain `>=` compare is sound — same as `mbox_call`).
#[inline]
fn deadline_ms(ms: u64) -> u64 {
    crate::arch::timer::cntpct() + crate::arch::timer::cntfrq().saturating_mul(ms) / 1000
}
#[inline]
fn expired(deadline: u64) -> bool {
    crate::arch::timer::cntpct() >= deadline
}

/// Spin until every bit in `mask` at `off` reads 0, or the deadline expires. Returns whether it cleared.
fn wait_clear(base: usize, off: usize, mask: u32, ms: u64) -> bool {
    let dl = deadline_ms(ms);
    while read32(base, off) & mask != 0 {
        if expired(dl) {
            return false;
        }
        core::hint::spin_loop();
    }
    true
}
/// Spin until any bit in `mask` at `off` reads 1, or the deadline expires. Returns whether it set.
fn wait_set(base: usize, off: usize, mask: u32, ms: u64) -> bool {
    let dl = deadline_ms(ms);
    while read32(base, off) & mask == 0 {
        if expired(dl) {
            return false;
        }
        core::hint::spin_loop();
    }
    true
}

/// Issue one command (`cmdtm` built from `cmd()` + response/flag bits) with argument `arg`, and wait for
/// completion. For a data command (`CMD_ISDATA`) the caller sets `BLKSIZECNT` first. Returns Ok on
/// CMD_DONE with no error; Err on a command timeout / CRC / index error, or on our own bounded timeout —
/// the caller (an init step) treats any Err as "this base failed" and the probe falls back / gives up.
fn send_command(base: usize, cmdtm: u32, arg: u32) -> Result<(), ()> {
    // Wait for BOTH the command AND data lines to be free before issuing. Waiting on DAT_INHIBIT
    // unconditionally (not just before a data/busy command) is load-bearing on metal: a preceding R1b
    // command (CMD7 SELECT) leaves the card asserting BUSY on DAT0, which the controller reflects as
    // DAT_INHIBIT=1 until it clears. Issuing the next command (CMD16) while that busy is still asserted
    // is rejected by a real Arasan/EMMC2 (→ init fails → on metal, where EMMC2 is the only card, M6g
    // fails). QEMU deasserts busy instantly so this window is invisible there; the wait closes it.
    if !wait_clear(base, STATUS, ST_CMD_INHIBIT | ST_DAT_INHIBIT, CMD_TIMEOUT_MS) {
        return Err(());
    }
    write32(base, INTERRUPT, 0xFFFF_FFFF); // clear any stale status
    write32(base, ARG1, arg);
    write32(base, CMDTM, cmdtm); // issues the command

    // Wait for command-complete OR any error. A card that isn't there never sets CMD_DONE — QEMU flags
    // CMD_TIMEOUT (an error bit) for a response-expecting command with no card, and may also set CMD_DONE,
    // so we test the error bits regardless of CMD_DONE.
    if !wait_set(base, INTERRUPT, INT_CMD_DONE | INT_ERR_ANY, CMD_TIMEOUT_MS) {
        return Err(());
    }
    let int = read32(base, INTERRUPT);
    if int & INT_ERR_ANY != 0 {
        write32(base, INTERRUPT, int); // W1C what we saw
        return Err(());
    }
    write32(base, INTERRUPT, INT_CMD_DONE);
    Ok(())
}

/// Check the R1 card-status word (RESP0, valid immediately after an R1 command completes) for
/// card-reported errors. `who` names the command for the one-line serial diagnostic. The controller's
/// interrupt error bits (checked in `send_command`) cover only link-level failures; this is the CARD's
/// own verdict — address out of range, write-protect violation, ECC failure, etc. Returns Err so the
/// caller surfaces a real `BlockError` instead of treating a card-rejected transfer as success.
fn r1_check(base: usize, who: &str) -> Result<(), ()> {
    let r1 = read32(base, RESP0);
    if r1 & R1_ERROR_MASK != 0 {
        serial_println!(":: M6g: {} R1 error status {:#010x} ::", who, r1);
        return Err(());
    }
    Ok(())
}

/// Read the four response registers (for R2 / 136-bit responses; also used to read R1/R3/R6/R7's RESP0).
#[inline]
fn read_resp(base: usize) -> [u32; 4] {
    [read32(base, RESP0), read32(base, RESP1), read32(base, RESP2), read32(base, RESP3)]
}

/// Extract CSD bit range `[hi:lo]` from a 136-bit R2 response. SDHCI strips the CRC byte and shifts the
/// 120-bit content right 8, so CSD bit `b` (b >= 8) lands at overall response bit `b-8` (the classic
/// off-by-8). `resp[i]` holds response bits `[32i+31 : 32i]`.
fn csd_bits(resp: &[u32; 4], hi: u32, lo: u32) -> u64 {
    let mut val = 0u64;
    let mut b = hi;
    loop {
        let r = b - 8; // CSD bit b -> response bit r
        let bit = (resp[(r / 32) as usize] >> (r % 32)) & 1;
        val = (val << 1) | bit as u64;
        if b == lo {
            break;
        }
        b -= 1;
    }
    val
}

/// Resolve a candidate base's SD base clock in Hz: CAPABILITIES[15:8] MHz if nonzero, else the VideoCore
/// mailbox GET_CLOCK_RATE for this base's clock id, else assume 100 MHz (logged). QEMU reports 52 MHz in
/// CAP so the mailbox/assume legs never run there.
fn base_clock(base: usize, clock_id: u32) -> u32 {
    let cap_mhz = (read32(base, CAPABILITIES) >> 8) & 0xFF;
    if cap_mhz != 0 {
        return cap_mhz * 1_000_000;
    }
    if let Some(hz) = crate::arch::mailbox::get_clock_rate(clock_id) {
        return hz;
    }
    serial_println!(":: M6g: base clock unknown (CAP=0, mailbox failed) — assuming 100 MHz ::");
    100_000_000
}

/// Program the SD clock to (at most) `target_hz` using the SDHCI-3 10-bit divided-clock mode
/// (SDCLK = base/(2·DIV)). Disables the SD clock, sets the divider + data-timeout + internal clock, waits
/// for the internal clock to stabilise, then re-enables the SD clock. Returns whether the clock stabilised.
fn set_clock(base: usize, base_hz: u32, target_hz: u32) -> bool {
    // Disable the SD clock before changing the divider.
    let c1 = read32(base, CONTROL1) & !C1_CLK_EN;
    write32(base, CONTROL1, c1);

    // DIV = ceil(base / (2*target)), clamped into the 10-bit field (>=1 so we never select bypass).
    let denom = target_hz.saturating_mul(2).max(1);
    let div = base_hz.div_ceil(denom).clamp(1, 0x3FF);
    // Freq select: low 8 bits -> [15:8], high 2 bits -> [7:6].
    let freq = ((div & 0xFF) << 8) | (((div >> 8) & 0x3) << 6);

    let mut c1 = read32(base, CONTROL1);
    c1 &= !0x0000_FFC0; // clear the old freq-select field ([15:8] + [7:6])
    c1 &= !(0xF << 16); // clear DATA_TOUNIT
    c1 |= freq;
    c1 |= 0xE << 16; // DATA_TOUNIT = max
    c1 |= C1_CLK_INTLEN;
    write32(base, CONTROL1, c1);

    if !wait_set(base, CONTROL1, C1_CLK_STABLE, CLK_STABLE_TIMEOUT_MS) {
        return false;
    }
    let c1 = read32(base, CONTROL1) | C1_CLK_EN;
    write32(base, CONTROL1, c1);
    true
}

/// Run the full SDHCI init ladder against `base` at 400 kHz identification clock, and if a card answers,
/// select it, learn its capacity from the CSD, raise the clock to transfer speed, and return the card.
/// Silent on failure (returns None) — the caller emits the single fallback / no-card line, so a failing
/// base never adds an unaccounted serial line. `clock_id` is the mailbox fallback for the base clock.
fn try_init(base: usize, clock_id: u32) -> Option<SdCard> {
    // 1. Full-controller software reset; wait for it to self-clear.
    write32(base, CONTROL1, read32(base, CONTROL1) | C1_SRST_HC);
    if !wait_clear(base, CONTROL1, C1_SRST_HC, RESET_TIMEOUT_MS) {
        return None;
    }
    // 2. Enable status latching (must be set or STATUS bits never appear in INTERRUPT); keep signals off
    //    (polled); clear any stale status.
    write32(base, IRPT_MASK, 0xFFFF_FFFF);
    write32(base, IRPT_EN, 0);
    write32(base, INTERRUPT, 0xFFFF_FFFF);
    // 3. Bus power: 3.3 V select + bus power on (CONTROL0 bits[11:8] = 0xF).
    write32(base, CONTROL0, (read32(base, CONTROL0) & !(0xF << 8)) | (0xF << 8));

    // 4. 400 kHz identification clock (data-timeout unit folded in by set_clock).
    let base_hz = base_clock(base, clock_id);
    if !set_clock(base, base_hz, 400_000) {
        return None;
    }

    // 5. CMD0 GO_IDLE (no response).
    send_command(base, cmd(0) | CMD_RESP_NONE, 0).ok()?;
    // 6. CMD8 SEND_IF_COND (R7, CRC+index check): 0x1AA = 2.7-3.6 V + check pattern 0xAA. The echo is the
    //    discriminator — a base with no card cannot echo 0x1AA (it times out or returns garbage).
    send_command(base, cmd(8) | CMD_RESP_48 | CMD_CRCCHK | CMD_IXCHK, 0x1AA).ok()?;
    if read32(base, RESP0) & 0xFFF != 0x1AA {
        return None;
    }
    // 7. ACMD41 loop (bounded ~1 s): CMD55 (APP_CMD, R1) then ACMD41 (SD_SEND_OP_COND, R3 — CRC AND index
    //    check DISABLED; the OCR has no valid CRC/index) with HCS + the 3.3 V window, until the card's
    //    power-up-busy bit (RESP0[31]) clears. ccs = RESP0[30] -> block vs byte addressing.
    let acmd41_deadline = deadline_ms(ACMD41_TIMEOUT_MS);
    let mut ocr;
    loop {
        send_command(base, cmd(55) | CMD_RESP_48 | CMD_CRCCHK | CMD_IXCHK, 0).ok()?;
        send_command(base, cmd(41) | CMD_RESP_48, 0x40FF_8000).ok()?;
        ocr = read32(base, RESP0);
        if ocr & (1 << 31) != 0 {
            break; // powered up
        }
        if expired(acmd41_deadline) {
            return None;
        }
        core::hint::spin_loop();
    }
    let block_addressing = ocr & (1 << 30) != 0; // ccs

    // 8. CMD2 ALL_SEND_CID (R2, CRC on, index off) — moves the card to identification state.
    send_command(base, cmd(2) | CMD_RESP_136 | CMD_CRCCHK, 0).ok()?;
    // 9. CMD3 SEND_RELATIVE_ADDR (R6) -> rca in RESP0[31:16].
    send_command(base, cmd(3) | CMD_RESP_48 | CMD_CRCCHK | CMD_IXCHK, 0).ok()?;
    let rca = read32(base, RESP0) >> 16;
    let rca_arg = rca << 16;

    // 10. CMD9 SEND_CSD (R2) — the card must be in stand-by (post-CMD3, pre-CMD7). Parse capacity.
    send_command(base, cmd(9) | CMD_RESP_136 | CMD_CRCCHK, rca_arg).ok()?;
    let resp = read_resp(base);
    let csd_structure = csd_bits(&resp, 127, 126);
    let (num_blocks, csd_version) = if csd_structure == 1 {
        // CSD v2 (SDHC/SDXC): C_SIZE = CSD[69:48]; capacity = (C_SIZE+1) * 512 KiB -> blocks = (C_SIZE+1)*1024.
        let c_size = csd_bits(&resp, 69, 48);
        ((c_size + 1) * 1024, 2u8)
    } else {
        // CSD v1 (SDSC): standard formula. blocks(512) = (C_SIZE+1) * 2^(C_SIZE_MULT+2) * 2^READ_BL_LEN / 512.
        let read_bl_len = csd_bits(&resp, 83, 80) as u32;
        let c_size = csd_bits(&resp, 73, 62);
        let c_size_mult = csd_bits(&resp, 49, 47) as u32;
        let mult = 1u64 << (c_size_mult + 2);
        let block_len = 1u64 << read_bl_len;
        let capacity = (c_size + 1) * mult * block_len;
        (capacity / 512, 1u8)
    };
    if num_blocks == 0 {
        return None;
    }

    // 11. CMD7 SELECT_CARD (R1b) -> transfer state.
    send_command(base, cmd(7) | CMD_RESP_48_BUSY | CMD_CRCCHK | CMD_IXCHK, rca_arg).ok()?;
    // 12. CMD16 SET_BLOCKLEN 512 (R1; SDSC semantics, harmless on SDHC where 512 is fixed).
    send_command(base, cmd(16) | CMD_RESP_48 | CMD_CRCCHK | CMD_IXCHK, 512).ok()?;
    // 13. Raise to transfer clock (<= 25 MHz).
    if !set_clock(base, base_hz, 25_000_000) {
        return None;
    }

    Some(SdCard { base, block_addressing, num_blocks, csd_version, rca_arg })
}

/// Probe the microSD: EMMC2 first (the metal path), then the legacy base (QEMU's card). On success,
/// register the block backend and log the identified line. Emits the fallback line at the EMMC2->legacy
/// transition (absent on metal, where EMMC2 wins) and one clean failure line if neither base answers.
/// Called once, synchronously on the BSP, after `start_aps` — single-threaded mailbox use, deterministic
/// serial placement (its lines land early, before the M6b demo). Never hangs boot: every wait is bounded.
pub fn probe() {
    if let Some(card) = try_init(EMMC2_BASE, CLK_ID_EMMC2) {
        finish(card);
        return;
    }
    // EMMC2 has no card in QEMU; announce the fallback (on metal EMMC2 succeeds and this line is absent).
    serial_println!(
        ":: M6g: EMMC2 @{:#x}: no card — falling back to SDHCI @{:#x} ::",
        EMMC2_BASE,
        LEGACY_BASE
    );
    if let Some(card) = try_init(LEGACY_BASE, CLK_ID_EMMC) {
        finish(card);
        return;
    }
    serial_println!(":: M6g: no SD card on either base — no block device registered ::");
}

/// Announce an identified card and register it as the block backend (read-only). The vendor/product tags
/// are cosmetic (shown only by the shell) — the loader path uses only the geometry.
fn finish(card: SdCard) {
    let mib = card.num_blocks * 512 / (1024 * 1024);
    serial_println!(
        ":: M6g: SD card @0x{:x} identified — {} blocks ({} MiB, CSD v{}) ::",
        card.base,
        card.num_blocks,
        mib,
        card.csd_version
    );
    let info = BlockDeviceInfo {
        slot_id: 0,
        block_size: 512,
        num_blocks: card.num_blocks,
        vendor: *b"BCM-SD  ",
        product: *b"microSD Card    ",
    };
    // WEDGE-10: install BEFORE flipping the block backend, so the first routed read finds a claimable
    // card rather than a `NotReady` — the same ordering the pre-loan code had.
    install(card);
    crate::drivers::block::register_sd(info);
}

/// Read one 512-byte block at `lba` into `buf` (>= 512 bytes) via a polled single-block CMD17. Returns
/// the number of bytes copied (512) on success. Backs `drivers::block::read_block` on the SD backend.
/// No cache maintenance: PIO into a normal kernel buffer, no DMA.
///
/// WEDGE-10 (F2): the card is CLAIMED (a loan, held by this frame) rather than locked — the polled
/// CMD17 ladder below runs with no lock held, so preempting this frame mid-transfer strands nobody.
/// See [`CARD`] and [`claim_for_io`].
pub fn read_block_512(lba: u64, buf: &mut [u8]) -> Result<usize, BlockError> {
    let card = claim_for_io()?;
    if lba >= card.num_blocks {
        return Err(BlockError::BadLba);
    }
    // SDSC uses byte addressing; guard that the byte offset fits the 32-bit ARG1 register.
    let arg = if card.block_addressing {
        lba as u32
    } else {
        let byte_off = lba.checked_mul(512).filter(|&b| b <= u32::MAX as u64).ok_or(BlockError::BadLba)?;
        byte_off as u32
    };
    let base = card.base;

    write32(base, INTERRUPT, 0xFFFF_FFFF); // clear stale status
    write32(base, BLKSIZECNT, (1 << 16) | 512); // one block, 512 bytes

    // CMD17 READ_SINGLE_BLOCK: R1 (CRC+index check) + data present + card->host direction.
    send_command(
        base,
        cmd(17) | CMD_RESP_48 | CMD_CRCCHK | CMD_IXCHK | CMD_ISDATA | CMD_DAT_DIR_READ,
        arg,
    )
    .map_err(|_| BlockError::Io)?;
    // The card answered at the link layer; now check ITS R1 verdict (out-of-range, ECC, ...) before
    // touching the FIFO — a card that rejected CMD17 will never fill the read buffer.
    r1_check(base, "CMD17").map_err(|_| BlockError::Io)?;

    // PIO in: wait for the block to be buffered, read 128 little-endian words, then wait transfer-complete.
    if !wait_set(base, INTERRUPT, INT_READ_RDY, DATA_TIMEOUT_MS) {
        return Err(BlockError::Io);
    }
    write32(base, INTERRUPT, INT_READ_RDY); // W1C
    let n = buf.len().min(512);
    for i in 0..128usize {
        let word = read32(base, DATA);
        let bytes = word.to_le_bytes();
        let off = i * 4;
        for (k, &b) in bytes.iter().enumerate() {
            if off + k < n {
                buf[off + k] = b;
            }
        }
    }
    if !wait_set(base, INTERRUPT, INT_DATA_DONE | INT_ERR_ANY, DATA_TIMEOUT_MS) {
        return Err(BlockError::Io);
    }
    let int = read32(base, INTERRUPT);
    write32(base, INTERRUPT, int); // W1C everything we saw
    if int & INT_ERR_ANY != 0 {
        return Err(BlockError::Io);
    }
    Ok(n)
}

/// U9: write one 512-byte block at `lba` from `buf` via a polled single-block CMD24 — the exact mirror of
/// `read_block_512` with three deltas: the command is WRITE_SINGLE_BLOCK (`cmd(24)`, direction host->card so
/// `CMD_DAT_DIR_READ` is OMITTED), the ready bit is Buffer-Write-Ready (`INT_WRITE_RDY`, bit 4) not
/// Buffer-Read-Ready (bit 5), and the FIFO loop PUSHES 128 little-endian words instead of draining them.
/// Backs `drivers::block::write_block` on the SD backend. `buf` shorter than 512 is zero-padded to the block
/// size (the block-layer convention) — the controller demands exactly 128 words or it stalls / corrupts the
/// block, so we always push a full sector. No cache maintenance (PIO from a normal kernel buffer, no DMA).
///
/// ⚠ This is the FIRST metal-risk WRITE path on the Pi 4 EMMC2 driver: QEMU's generic-sdhci models the PIO
/// write FIFO, but silicon timing (buffer-write-ready latency, DAT0 programming-busy) is only proven on metal.
///
/// WEDGE-10 (F2): as with the read twin, the card is CLAIMED for the duration — the CMD24 ladder, the
/// 500 ms programming-busy wait and the CMD13 verdict all run with no lock held. This is the longest
/// hold in the driver (~1.3 s of bounded deadlines) and so the one that made F2 reachable at all.
pub fn write_block_512(lba: u64, buf: &[u8]) -> Result<(), BlockError> {
    let card = claim_for_io()?;
    if lba >= card.num_blocks {
        return Err(BlockError::BadLba);
    }
    // SDSC uses byte addressing; guard that the byte offset fits the 32-bit ARG1 register (as the read path does).
    let arg = if card.block_addressing {
        lba as u32
    } else {
        let byte_off = lba.checked_mul(512).filter(|&b| b <= u32::MAX as u64).ok_or(BlockError::BadLba)?;
        byte_off as u32
    };
    let base = card.base;

    write32(base, INTERRUPT, 0xFFFF_FFFF); // clear stale status
    write32(base, BLKSIZECNT, (1 << 16) | 512); // one block, 512 bytes

    // CMD24 WRITE_SINGLE_BLOCK: R1 (CRC+index check) + data present + host->card direction (no DAT_DIR_READ).
    send_command(
        base,
        cmd(24) | CMD_RESP_48 | CMD_CRCCHK | CMD_IXCHK | CMD_ISDATA | CMD_DAT_DIR_WRITE,
        arg,
    )
    .map_err(|_| BlockError::Io)?;
    // Mirror of the CMD17 check: the card's R1 verdict (write-protect, out-of-range, card-locked, ...)
    // before pushing data — a card that rejected CMD24 will never accept the FIFO words.
    r1_check(base, "CMD24").map_err(|_| BlockError::Io)?;

    // PIO out: wait for the write buffer to be ready, push 128 little-endian words, then wait transfer-complete.
    if !wait_set(base, INTERRUPT, INT_WRITE_RDY, DATA_TIMEOUT_MS) {
        return Err(BlockError::Io);
    }
    write32(base, INTERRUPT, INT_WRITE_RDY); // W1C
    let n = buf.len().min(512);
    for i in 0..128usize {
        let off = i * 4;
        // Assemble one LE word from up to four source bytes; bytes past `n` (a short buffer) are zero-padded.
        let mut word = [0u8; 4];
        for (k, slot) in word.iter_mut().enumerate() {
            if off + k < n {
                *slot = buf[off + k];
            }
        }
        write32(base, DATA, u32::from_le_bytes(word));
    }
    if !wait_set(base, INTERRUPT, INT_DATA_DONE | INT_ERR_ANY, DATA_TIMEOUT_MS) {
        return Err(BlockError::Io);
    }
    let int = read32(base, INTERRUPT);
    write32(base, INTERRUPT, int); // W1C everything we saw
    if int & INT_ERR_ANY != 0 {
        return Err(BlockError::Io);
    }

    // The transfer is done at the LINK layer, but the card is now busy programming flash (DAT0 low) and
    // programming-phase failures (CARD_ECC_FAILED, generic ERROR, WP_ERASE_SKIP) are only reported by a
    // LATER SEND_STATUS — without this, a write the card ultimately discarded still returns Ok. Wait out
    // programming-busy under the spec's write-timeout bound (send_command's own DAT_INHIBIT wait is only
    // 100 ms — too short for a legal 250 ms busy), then fetch the card's post-programming verdict.
    if !wait_clear(base, STATUS, ST_DAT_INHIBIT, PROG_BUSY_TIMEOUT_MS) {
        serial_println!(":: M6g: CMD24 programming-busy timeout ::");
        return Err(BlockError::Io);
    }
    send_command(base, cmd(13) | CMD_RESP_48 | CMD_CRCCHK | CMD_IXCHK, card.rca_arg)
        .map_err(|_| BlockError::Io)?;
    r1_check(base, "CMD13").map_err(|_| BlockError::Io)?;
    Ok(())
}
