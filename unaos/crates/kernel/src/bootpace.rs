// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// BPACE — the boot-phase timing ledger.
//
// The question it exists to answer: *where does the boot spend its wall clock?* Until now the only
// numbers a metal sitting produced were a stopwatch and a feeling ("it sits for a long time before
// the desktop"), which cannot separate firmware from heap from ACPI from USB enumeration from the
// SCSI bring-up. A settle constant cannot be trimmed against a feeling. This module makes ONE metal
// boot yield both the post-BOT-CBW baseline and the per-phase breakdown, so the next arc's timing
// work is arithmetic instead of guesswork.
//
// Modelled on `bootlog.rs` (heap-free static ring, `record(tag)`, growth-triggered serial re-dump),
// with two deliberate differences:
//
//   1. Entries are stamped with `arch::now_cycles()`, NOT `arch::ms()`. `arch::ms()` is the 1 kHz
//      APIC tick, and the tick does not exist until `apic::calibrate` runs deep inside boot — so it
//      reads ~0 for EVERY phase before calibration and cannot timestamp the part of the boot we
//      most need to measure. `now_cycles()` is rdtsc on x86 and CNTVCT_EL0 on aarch64: free-running
//      from reset, arch-neutral, and independent of EFLAGS.IF / the timer ISR.
//
//   2. The cycles→milliseconds conversion happens at PRINT time, not at record time, because the
//      counter's rate is not known until calibration. If the rate is still unknown when the ledger
//      prints, it prints RAW COUNTER TICKS with a `cy` suffix and `hz=0` — an honest "I do not know
//      how fast this counter runs" rather than a fabricated millisecond.
//
// Overflow policy is drop-NEWEST (unlike `bootlog`'s drop-oldest). This ledger's subject is the
// BOOT; the entries nearest a late hot-plug are the expendable ones, and losing `entry` would
// destroy the origin every other `t=` is measured against. A saturating `dropped=` count on the
// total line keeps the truncation visible instead of silent.
//
// ── The instrument-baseline law (docs/dev/OS/01_BOOT_HAL/bootpace.md) ───────────────────────────
// A witness is not finished until someone has written down what it reads in the healthy case, what
// it reads when the mechanism did NOT run, and shown that those two differ. For this ledger:
//
//   * HEALTHY — the full ledger prints: every tag from `entry` through `ftdi-up` present, `t=`
//     strictly nondecreasing, `gui=` in the tens of seconds, and `ftdi=` strictly greater than
//     `gui=`. That last inequality is structural, not incidental: on a GUI build the handoff
//     happens BEFORE the service loop that runs enumeration/storage/FTDI starts at all, so every
//     main-loop tag necessarily lands after `gui`. On a `usbdebug` build the GUI is never reached
//     and the same ledger reads `gui=none` with `ftdi=` present.
//   * DID NOT RUN (a) — no FTDI cable attached: `ftdi-up` never records, `total` reads
//     `ftdi=none`, and no BPACE block reaches a second host at all. Absence of the block IS the
//     reading; a ledger that "looks fine" on a wire nobody is watching proves nothing.
//   * DID NOT RUN (b) — a `UNAOS_SKIP_XHCI=1` build: `entry/heap/acpi/calib/smp/sched/gui` still
//     print and `gui=` still carries a number, while the ENTIRE `pci-enter … xhci-settle`
//     subdivision, every `enum:p*`, `stor-*`, `bot-first`, `fat-mount`, `fr-flush` and `ftdi-up`
//     are ABSENT — `pci::init` is not called at all on that build, so `sched` is followed directly
//     by `gui`. That asymmetry is what proves the USB tags are reporting the USB path and not the
//     recorder's own liveness.
//
// The three readings differ, so the instrument can falsify. Adds no env knob, by design: a knob
// known to `arroyo` but unknown to `builder/` ships the feature disabled with every gate green, and
// that has cost this project two arcs already.
//
// ── BOOTPACE M4: the subdivided PCI/USB span ────────────────────────────────────────────────────
// The first metal ledger (2026-07-30) read `xhci-settle t=7746ms d=7289ms` — 92% of the time to
// serial console, in ONE bucket, with no stamp anywhere between `smp` (t=456 ms) and it. A `d=` is
// always the delta from the PREVIOUS stamp, so that number was never a settle measurement: it was
// the scheduler, the tick-rate window, the EHCI HID bring-up, two PCI config-space walks, the
// BIOS→OS handoff, the halt/HCRST/CNR chain, the ring/interrupter programming AND the settle,
// summed. No trim could be aimed at it. M4 adds thirteen stamps so each of those becomes a line of
// its own; the tags follow the code's own names (`portsw` is PORTSW-1, `xhci-cnr` is the CNR wall
// of usb_xhci.md §1a, `ehci-hid` is the EHCI-3 driver) rather than an invented taxonomy.
//
// HEALTHY (x86, GUI build, xHCI present) reads, in order:
//
//   entry heap acpi calib smp sched pci-enter ehci-hid ehci-hid-done pci-scan portsw
//   xhci-handoff xhci-halt xhci-hcrst xhci-cnr xhci-ptrs xhci-run xhci-portpwr xhci-settle
//   enum:p* … pci-usb gui … stor-* bot-first fat-mount fr-flush ftdi-up
//
// with `xhci-settle`'s `d=` now equal to the settle constant ALONE. `pci-usb` still lands after
// `xhci-settle` and the first `enum:p*`, because `start()` kicks enumeration before `pci::init`
// returns — that ordering is real, not a defect, and the subdivision makes it legible instead of
// confusing. Every phase that contains a `hw_wait_budget()`-derived wait (`ehci-hid`, the handoff,
// halt, HCRST, CNR, the ring programming, the run handshake) is stamped on BOTH sides, so a boot
// that dies inside one of them still names the phase it entered.
//
// SKIPPED (`UNAOS_SKIP_XHCI=1`) reads `… smp sched gui …`: not one of the thirteen new tags
// appears, because none of their call sites is reached. The two readings differ in 13 lines, so
// the subdivision itself can be falsified — a build with USB compiled out that still printed
// `xhci-cnr` would convict the stamps of reporting the recorder rather than the controller.
//
// ── Read-only accessors (GPACE) ─────────────────────────────────────────────────────────────────
// `cycles_of` / `last_stamp` / `origin_hz` let an accumulator OUTSIDE this module open a span at
// the exact instant of a stamp already in the ring, and convert it with the same rate the ledger
// uses. They are the reason a subdivision such as GPACE (`arch/x86_64/pci.rs`) can claim its total
// equals the enclosing `pci-usb d=` by CONSTRUCTION rather than by eyeball. None of them records,
// grows the ring, or perturbs `dropped=`: the ledger stays exactly as long as the stamps made it,
// which matters because drop-NEWEST would spend any growth on the late boot tags.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

/// Ring capacity. Sized for the fixed boot phases plus a start/done pair for every root port a real
/// controller exposes (the rMBP's Panther Point xHCI reports MaxPorts=4..8; QEMU's model reports
/// more), with slack. 64 entries × 16 bytes = 1 KiB of `.bss`, paid once.
///
/// BOOTPACE M4 headroom check (the `dropped=` field is only honest if someone did the arithmetic):
/// the M4 subdivision takes the fixed phases from 15 to 28. The first metal ledger recorded n=18
/// with two ports enumerated; the same boot under M4 records 31, leaving 33 free slots — room for
/// 16 further port enumerations (2 stamps each) before `dropped=` could become non-zero. That is
/// well beyond MaxPorts on any controller this kernel has met, so CAP stays 64 and no capacity
/// change rides this arc.
const CAP: usize = 64;

/// One phase stamp: the free-running counter at the moment it was reached, and its static tag.
type Entry = (u64, &'static str);

struct Ring {
    slots: [Entry; CAP],
    /// Number of valid entries, oldest first (drop-newest: entries never move).
    len: usize,
}

impl Ring {
    const fn new() -> Self {
        Ring { slots: [(0, ""); CAP], len: 0 }
    }
}

static RING: Mutex<Ring> = Mutex::new(Ring::new());

/// Stamps refused because the ring was full. Reported as `dropped=` so a truncated ledger is never
/// mistaken for a complete one.
static DROPPED: AtomicUsize = AtomicUsize::new(0);

/// The counter value of the FIRST stamp — the origin every `t=` is measured from. Held separately
/// from `slots[0]` so the origin survives any future change of overflow policy.
static ORIGIN: AtomicU64 = AtomicU64::new(0);

/// Per-port enumeration tags, as `&'static str` literals so an entry stays `Copy` and the ring stays
/// heap-free (a formatted tag would need an allocator, and the earliest stamps predate the heap).
/// xHCI root ports are 1-based; index 0 is never used by `record_port`.
const ENUM_TAGS: [&str; 17] = [
    "enum:p0", "enum:p1", "enum:p2", "enum:p3", "enum:p4", "enum:p5", "enum:p6", "enum:p7",
    "enum:p8", "enum:p9", "enum:p10", "enum:p11", "enum:p12", "enum:p13", "enum:p14", "enum:p15",
    "enum:p16",
];
const ENUM_DONE_TAGS: [&str; 17] = [
    "enum:p0-done", "enum:p1-done", "enum:p2-done", "enum:p3-done", "enum:p4-done",
    "enum:p5-done", "enum:p6-done", "enum:p7-done", "enum:p8-done", "enum:p9-done",
    "enum:p10-done", "enum:p11-done", "enum:p12-done", "enum:p13-done", "enum:p14-done",
    "enum:p15-done", "enum:p16-done",
];

/// Record a boot phase. `tag` is a short static identifier — see the table in
/// `docs/dev/OS/01_BOOT_HAL/bootpace.md`. Lock-light and allocation-free: one `now_cycles()` read
/// and a couple of array writes under a leaf mutex that is never held across any other lock or any
/// I/O. Safe from event context (the xHCI stamps run there), from before the heap exists (the
/// `entry` stamp does), and from any arch.
///
/// Unlike `bootlog::record` this deliberately does NOT paint the panel: the ledger's readers are
/// the serial/FTDI wire and the post-boot log, and a per-phase panel line during enumeration would
/// perturb the very boot it is measuring.
pub fn record(tag: &'static str) {
    let c = crate::arch::now_cycles();
    let mut r = RING.lock();
    let n = r.len;
    if n == CAP {
        DROPPED.fetch_add(1, Ordering::Relaxed);
        return;
    }
    if n == 0 {
        ORIGIN.store(c, Ordering::Relaxed);
    }
    r.slots[n] = (c, tag);
    r.len = n + 1;
}

/// Record the start (`done == false`) or the end (`done == true`) of one root port's enumeration.
/// A port number outside the literal table degrades to `enum:p?` rather than being dropped — the
/// timing is still true even when the port identity has to be read off the neighbouring
/// `xHCI: === Enumerating Port N ===` line.
pub fn record_port(port: u8, done: bool) {
    let table = if done { &ENUM_DONE_TAGS } else { &ENUM_TAGS };
    let tag = match table.get(port as usize) {
        Some(t) => *t,
        None => {
            if done {
                "enum:p?-done"
            } else {
                "enum:p?"
            }
        }
    };
    record(tag);
}

/// One-shot latch: record `tag` the first time this call site is reached and never again. Used for
/// the phases that sit inside a loop or a per-transaction path (`bot-first`, `fr-flush`), where the
/// ledger wants the FIRST occurrence and nothing else.
pub fn record_once(latch: &AtomicUsize, tag: &'static str) {
    if latch.swap(1, Ordering::Relaxed) == 0 {
        record(tag);
    }
}

/// The counter value of the most recent stamp whose tag equals `tag`, or `None` when that tag has
/// not been recorded on this boot.
///
/// READ-ONLY by construction: it takes the ring's leaf mutex, copies one `u64` out and releases it.
/// No stamp is added, `len` does not move, and the `dropped=` arithmetic is untouched. That
/// restraint is not politeness — the metal ledger sits at `n=31` of `CAP=64` with a drop-NEWEST
/// policy, so an accessor that grew the ring would silently destroy the LATE boot tags (`stor-*`,
/// `fat-mount`, `fr-flush`, `ftdi-up`) this ledger exists to carry.
///
/// It exists so a phase accumulator in another module can open a span at the EXACT instant of a
/// stamp already in this ledger, rather than "shortly after" it. That exactness is what makes a
/// self-check against a BPACE `d=` mean something: with an approximate origin, a disagreement
/// between the two instruments is a shrug instead of a conviction.
pub fn cycles_of(tag: &str) -> Option<u64> {
    let r = RING.lock();
    let n = r.len;
    r.slots[..n].iter().rev().find(|(_, t)| *t == tag).map(|(c, _)| *c)
}

/// The most recent stamp of ANY tag — its counter value and its tag — or `None` before the first
/// `record`.
///
/// The companion `cycles_of` needs to make a self-check exact rather than merely close. BPACE
/// computes every `d=` against the IMMEDIATELY PRECEDING stamp, whichever tag that turns out to be;
/// so an accumulator whose total must equal the next `d=` *by construction* has to anchor on the
/// last stamp as it actually is, not on the tag it expected to find there. Anchoring on a guessed
/// tag makes the equality a coincidence of topology — true on one machine, quietly false on the
/// next, with no reading that would reveal the difference.
pub fn last_stamp() -> Option<(u64, &'static str)> {
    let r = RING.lock();
    if r.len == 0 { None } else { Some(r.slots[r.len - 1]) }
}

/// The rate of `arch::now_cycles()` in Hz, or 0 when it is not (yet) known — the very value this
/// ledger converts its own `t=`/`d=` with.
///
/// Exposed so an out-of-module accumulator converts against the LEDGER's clock instead of reaching
/// into an arch-specific `tsc_hz()`/`cntfrq()` of its own. A self-check between two instruments is
/// only meaningful when both divided by the same number; two rates make a disagreement
/// uninterpretable, and 0 here means both must print raw ticks rather than fabricate a millisecond.
pub fn origin_hz() -> u64 {
    counter_hz()
}

/// The counter value of the `entry` stamp — the zero every `t=`/`since-entry` is measured from —
/// or 0 when no stamp has been recorded yet. Exposed for the same reason as [`origin_hz`]: an
/// out-of-module timestamp (the `logts` line prefix) must subtract the LEDGER's origin, not invent
/// its own, or its absolute column disagrees with every BPACE/GPACE figure by the firmware+
/// bootloader duration the raw counter includes.
pub fn origin_cycles() -> u64 {
    ORIGIN.load(Ordering::Relaxed)
}

/// Copy the buffered stamps (oldest→newest) into `out`, returning how many were written. Snapshots
/// under the lock and releases it BEFORE the caller prints, so serial/FTDI I/O never runs while the
/// ring is held.
fn snapshot(out: &mut [Entry]) -> usize {
    let r = RING.lock();
    let n = r.len.min(out.len());
    out[..n].copy_from_slice(&r.slots[..n]);
    n
}

/// The rate of `arch::now_cycles()` in Hz, or 0 when it is not (yet) known.
///
/// Deliberately NOT the xHCI driver's `cycles_per_ms()` idiom, which substitutes a 2 GHz guess
/// before calibration. A guess is the right answer for a SETTLE (a wrong guess makes a settle
/// longer or shorter, never unsound); it is the wrong answer for a MEASUREMENT, where it would turn
/// "I do not know" into a fabricated millisecond that a later arc would then trim a real constant
/// against. Zero here means the ledger prints raw counter ticks instead.
fn counter_hz() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        // Calibrated against the ACPI PM timer by `apic::calibrate`; 0 until then (and 0 forever if
        // the PM timer was absent or the calibration was rejected as insane).
        crate::arch::apic::tsc_hz()
    }
    #[cfg(target_arch = "aarch64")]
    {
        // CNTFRQ_EL0 — the generic timer's declared rate, architecturally valid from reset.
        crate::arch::timer::cntfrq()
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        0
    }
}

/// A ledger duration rendered in the counter's honest unit: milliseconds (`ms`) when the timebase
/// rate is known, raw counter ticks (`cy`) when it is not, and `none` when the phase never
/// happened at all.
struct Dur {
    raw: Option<u64>,
    hz: u64,
}

impl core::fmt::Display for Dur {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.raw {
            None => write!(f, "none"),
            Some(v) if self.hz >= 1000 => write!(f, "{}ms", v / (self.hz / 1000)),
            Some(v) => write!(f, "{}cy", v),
        }
    }
}

/// Last ledger length surfaced by `service_dump` (sentinel so the first call always prints).
static LAST_DUMPED_LEN: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Main-loop SERVICE: re-print the WHOLE ledger whenever it has grown since the last print.
///
/// Re-emitting the full block on growth (rather than one line per stamp, or a single one-shot dump)
/// is load-bearing, for a reason specific to how this kernel reaches a second host. Every line the
/// kernel prints before the FTDI console arms lands in the 64 KiB capture ring (`drivers::ftdi`,
/// drop-oldest) and is replayed out the wire once the console is live — but a long boot overflows
/// that ring, and the OLDEST lines (the early phase stamps, exactly the ones nobody can re-measure)
/// are the ones it discards. Because the block is reprinted in full every time it grows, the LAST
/// block always rides the live wire AFTER `ftdi-up`, at the tail of the log, where it cannot be lost
/// to ring overflow. Bounded by construction: at most one block per recorded phase.
///
/// Deliberately ungated — no Cargo feature, no env knob. The build that reaches metal
/// (`./arroyo esp-x86`) carries neither `witness` nor `usbdebug`, and a ledger that only exists in
/// the builds nobody boots on hardware is not an instrument.
pub fn service_dump() {
    // CLOCK-X1 pay-as-you-go (GR18): deliver the deferred clock verdict. It rides HERE rather than
    // at any of the three service-loop bodies because this function is the one call all three make
    // ungated — the BSP GUI loop, the `usbdebug` loop and the SCHED-X86 `x86_usb_pump` task — so the
    // witness reaches the media build that boots on the bench, which is the whole reason this
    // ledger is ungated in the first place. One relaxed load per pass once the verdict has printed;
    // the early-return below must NOT precede it, since the ledger stops growing long before the
    // boot ends and the verdict would then never be delivered on a quiet boot.
    #[cfg(target_arch = "x86_64")]
    crate::arch::syscall::clock_x1_poll();

    // PAYGO-TERM (GR18): the witness battery's service-pass taker, riding here for CLOCK-X1's exact
    // reason and no other. A deferred `[wc-g]`/`[wc-d]` sample used to have only one taker — the next
    // composite — and Boot V showed that is not a taker at all on a boot that goes idle: the last
    // composite was at 13 776 ms against a 15 000 ms threshold, `x86_render_service` blocks on its
    // event channel, and the console window sat `state=waiting` for the remaining 210 s. This is the
    // hook that reaches every x86 service lane ungated, so a matured deferral gets taken whether or
    // not anything is presenting. Self-throttled to 4 Hz; one relaxed load per pass otherwise. Folds
    // to an empty fn without `witness` + `wcg-paygo`, and does not exist on aarch64.
    #[cfg(target_arch = "x86_64")]
    crate::video::wm::paygo_service();

    let mut buf = [(0u64, ""); CAP];
    let n = snapshot(&mut buf);
    if LAST_DUMPED_LEN.swap(n, Ordering::AcqRel) == n {
        return;
    }
    let hz = counter_hz();
    let origin = ORIGIN.load(Ordering::Relaxed);
    let mut prev = origin;
    for (c, tag) in &buf[..n] {
        let t = c.wrapping_sub(origin);
        let d = c.wrapping_sub(prev);
        serial_println!(
            ":: BPACE: {} t={} d={} ::",
            tag,
            Dur { raw: Some(t), hz },
            Dur { raw: Some(d), hz }
        );
        prev = *c;
    }
    let at = |want: &str| -> Option<u64> {
        buf[..n]
            .iter()
            .find(|(_, tag)| *tag == want)
            .map(|(c, _)| c.wrapping_sub(origin))
    };
    serial_println!(
        ":: BPACE: total gui={} ftdi={} n={} dropped={} hz={} result=LEDGER ::",
        Dur { raw: at("gui"), hz },
        Dur { raw: at("ftdi-up"), hz },
        n,
        DROPPED.load(Ordering::Relaxed),
        hz
    );
}
