#[macro_use]
pub mod serial;
pub mod gdt;
pub mod interrupts;
pub mod apic;
pub mod acpi;
pub mod acpi_power;
pub mod percpu;
pub mod smp;
pub mod sched;
pub mod syscall;
pub mod pci;
pub mod memory;
/// WINX-2: the x86_64 ring-3 ELF64 loader (validation + `PT_LOAD` mapping with per-segment W^X).
pub mod elf;

pub fn init() {
    // BPACE HPACE-1: `core-init d=` is the interval from the WC retype to here — on the current
    // ordering that is the `WRITER` seed and its one line; if `set_framebuffer_wc` is ever hoisted
    // ABOVE `fbcon::init` in `kernel_main`, the console's full-surface clear lands in this bucket
    // instead of in `fb-wc`. Which bucket carries the big number therefore SAYS which ordering the
    // build has, without anyone having to read the source. See bootpace.md §11.
    crate::bootpace::record("core-init");
    gdt::init();
    interrupts::init_idt();
    // Pure local-APIC system: silence the legacy 8259 PIC, then software-enable the local
    // APIC (timer heartbeat + spurious vector; LINT0 masked, LINT1=NMI) before enabling
    // interrupts. Input is USB-HID via the xHCI MSI-X path — no PS/2, no PIT, no I/O APIC.
    interrupts::disable_legacy_pic();
    apic::init();
    // Per-CPU data for the BSP (logical CPU 0). Must precede `sti` so the timer/IPI handlers
    // can resolve `this_cpu()` via the GS base.
    percpu::init_cpu(0, apic::apic_id_u32());
    // U1a: SYSCALL/SYSRET MSRs + NX/SMEP for the BSP. After gdt (STAR needs the selectors) and
    // percpu (GS base + KERNEL_GS_BASE); before `sti`.
    syscall::init();
    // WXN-x86 M1: the PDPT-level NX sweep — the first NX bit this kernel puts in its own map. Sits
    // HERE, and only here: after `syscall::init` because with EFER.NXE clear bit 63 is a RESERVED
    // bit (setting it would fault the next translation of ANY kind, not just a fetch), and before
    // `wx_audit_report` so the existing WXAUDIT census line IS the success signature rather than a
    // stale "before" a reader has to reconcile with a second one. Before `wx_probe_report` too, so
    // the WXPROBE `map:` lines are a post-sweep readback whose `e=` fields must be bit-identical to
    // the pre-sweep capture (no leaf is written) while their folded `fx=` fields flip to 0 outside
    // the spared GiBs (the parents did change). Needs no allocator — a PDPT-level sweep creates no
    // tables — which is what lets it run this early, and interrupts are still masked.
    memory::wxn_pdpt_sweep();
    // WXAUDIT: audit the live map now that EFER.NXE and CR4.SMEP are set — before this point an NX
    // bit in a PTE means nothing to the hardware, so an audit here would be reporting on a protection
    // that is not yet armed. Read-only walk; publishes the negative control and the map's W^X census.
    memory::wx_audit_report();
    // WXPROBE: the read-only reconnaissance the WXN-x86 split is designed from — leaf level and raw
    // bits at the six addresses the split must classify, the control registers that decide its flush
    // and its NX semantics, and the kernel's own PT_LOAD layout via `__ehdr_start`. Sits here rather
    // than anywhere else for the same reason the audit does (EFER.NXE/CR4.SMEP are armed, the heap
    // is not yet created, interrupts are still masked) and directly after it so the census and the
    // specific addresses land adjacent in one capture. Writes nothing.
    memory::wx_probe_report();
    x86_64::instructions::interrupts::enable();
}

pub fn hlt_loop() -> ! {
    loop {
        hlt();
    }
}

pub fn hlt() {
    x86_64::instructions::hlt();
}

/// Counterpart to aarch64's framebuffer cache-clean (`DC CVAC` + `DSB`). VPERF-WC made the x86
/// framebuffer mapping Write-Combining (`memory::set_framebuffer_wc`), so CPU stores to it now
/// buffer in the WC combining buffers and drain only on a store fence / serializing event /
/// buffer pressure (SDM Vol. 3A §11.3.1). A streaming console self-evicts under pressure, but the
/// LAST partial WC buffer before the CPU idles has no eviction trigger — e.g. the tail of the
/// panic screen ahead of `hlt_loop()` — so every present point must drain explicitly. `SFENCE` is
/// exactly that drain: it forces all prior WC stores globally visible. This rides the same seam
/// the Pi's cache-clean uses (`FrameBuffer::flush_range`/`flush_all`, called by fbcon's
/// per-print `flush_dirty`, the panic path's `flush_all`, and `Screen`'s per-frame present), so
/// every existing flush call site is a real drain point. Range-independent (SFENCE drains all WC
/// buffers); negligible cost next to the blit it follows.
#[inline]
pub fn flush_framebuffer_range(_addr: usize, _len: usize) {
    // SAFETY: SFENCE has no preconditions; it only orders/drains stores.
    unsafe { core::arch::asm!("sfence", options(nostack, preserves_flags)) };
}

/// Monotonic tick counter since boot (the local-APIC timer heartbeat). Arch-neutral entry
/// point used by drivers and the scheduler for coarse timing (e.g. TCP retransmission RTO,
/// `sleep_ticks`). Once the timer is calibrated against the ACPI PM timer (see `apic::calibrate`)
/// the tick runs at a real 1 kHz — i.e. one tick per millisecond — on any machine; before
/// calibration it is steady but uncalibrated (~0.8 ms/tick under QEMU).
pub fn ticks() -> u64 {
    apic::ticks()
}

/// Milliseconds since boot. The calibrated APIC tick is 1 kHz, so this is the tick count directly;
/// before calibration it degrades to the raw (~1 ms) tick count. Arch-neutral (mirrors aarch64).
#[inline]
pub fn ms() -> u64 {
    apic::ticks()
}

/// Convert a millisecond duration to local-APIC timer ticks for `sched::sleep_ms`. The heartbeat is
/// calibrated to `apic::TICK_HZ` (1000 Hz), so one tick is one millisecond and this is `ms *
/// TICK_HZ / 1000`. Deriving it from `TICK_HZ` (rather than returning `ms` outright) keeps the
/// conversion honest if the tick rate is ever retuned, and localises the ms<->tick relationship to
/// one place. Before calibration the tick is ~0.8 ms under QEMU, so a `sleep_ms` runs proportionally
/// short — the same graceful degradation `ms()`/`ticks()` already carry. (x86-only, alongside the
/// scheduler it serves; aarch64 has no scheduler yet.)
#[inline]
pub fn ms_to_ticks(ms: u64) -> u64 {
    ms.saturating_mul(apic::TICK_HZ) / 1000
}

/// Free-running CPU cycle counter (rdtsc). Invariant on Nehalem and later (incl. the Ivy Bridge
/// MacBookPro10,1): a constant rate across P-/C-/T-states, and — unlike `ticks()` — it advances
/// regardless of EFLAGS.IF or whether the APIC-timer ISR runs. `apic::calibrate` measures its
/// absolute rate against the ACPI PM timer (there is no CPUID leaf 0x15/0x16 before Skylake); use
/// `hw_wait_budget()` for a wall-clock deadline in these units.
#[inline]
pub fn now_cycles() -> u64 {
    // SAFETY: RDTSC has no preconditions at ring 0; it is gated only by CR4.TSD (a ring-3
    // restriction we never set), never by the interrupt flag.
    unsafe { core::arch::x86_64::_rdtsc() }
}

/// Wall-clock seconds a hardware busy-wait may burn before it is treated as wedged.
const HW_WAIT_SECONDS: u64 = 2;

/// Fixed fallback busy-wait budget in `now_cycles()` (rdtsc) units, used before/without calibration.
/// 2.5e9 cycles ≈ [0.5 s, 2.5 s] across 1–5 GHz parts (~1.1 s at a 2.3 GHz Ivy Bridge base) and
/// ≈2.5 s under QEMU/TCG: long enough that a healthy controller's µs-scale handshakes never trip
/// it, short enough that a wedged status bit fails fast instead of looking frozen on a serial-less
/// laptop.
pub const HW_WAIT_BUDGET: u64 = 2_500_000_000;

/// Busy-wait budget in `now_cycles()` (rdtsc) units. Once the TSC is calibrated this is an honest
/// wall-clock `HW_WAIT_SECONDS` (`tsc_hz * seconds`); before calibration (or if it failed / no PM
/// timer) it falls back to the fixed `HW_WAIT_BUDGET` guess. Callers pass this to `wait_until`.
#[inline]
pub fn hw_wait_budget() -> u64 {
    let hz = apic::tsc_hz();
    if hz != 0 {
        hz.saturating_mul(HW_WAIT_SECONDS)
    } else {
        HW_WAIT_BUDGET
    }
}

/// WEDGE-7 — the RAII form of [`without_interrupts`], for spans that cannot be expressed as a
/// closure. Twin of the aarch64 `IrqMask`; same save/restore semantics, so an inner guard restores
/// to "still masked" rather than unconditionally re-enabling. Needed because `video/wm.rs` compiles
/// on both arches and must hold the mask exactly as long as a lock guard whose scope the caller
/// owns (see `video::wm::table`).
pub struct IrqMask(bool);

/// WEDGE-8 (F3): true when interrupts are masked on this core (`RFLAGS.IF` clear). Twin of the
/// aarch64 `irqs_masked`; the block layer uses it to refuse a masked wait on the xHCI loan.
#[inline]
pub fn irqs_masked() -> bool {
    !x86_64::instructions::interrupts::are_enabled()
}

impl IrqMask {
    #[inline]
    pub fn new() -> Self {
        let was_enabled = x86_64::instructions::interrupts::are_enabled();
        x86_64::instructions::interrupts::disable();
        IrqMask(was_enabled)
    }
}

impl Default for IrqMask {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for IrqMask {
    #[inline]
    fn drop(&mut self) {
        if self.0 {
            x86_64::instructions::interrupts::enable();
        }
    }
}

pub fn without_interrupts<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    x86_64::instructions::interrupts::without_interrupts(f)
}
