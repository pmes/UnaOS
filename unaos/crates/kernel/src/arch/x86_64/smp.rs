// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// SMP application-processor (AP) bring-up.
//
// APs come out of reset (and out of a SIPI) in 16-bit real mode at CS:IP = (vector<<8):0, i.e.
// physical address vector*0x1000. There is no shortcut on x86: to run 64-bit Rust an AP must
// walk real -> protected -> long mode itself. So we copy a small trampoline to a low,
// identity-mapped page (0x8000) and kick each AP with the architectural INIT-SIPI-SIPI sequence.
//
// The trampoline is fully position-dependent on 0x8000 (it bakes that base into every absolute
// reference as `0x8000 + (label - start)`), which means it contains NO relocations and can be
// copied byte-for-byte. It sets up a temporary GDT (32-bit + 64-bit descriptors), enables PAE +
// the BSP's page tables (CR3) + long mode, and jumps to `ap_entry` on a per-AP stack. From there
// each AP loads its own per-CPU GDT/TSS (gdt::init_cpu), the shared IDT, and its own local APIC,
// then idles — the BSP keeps driving xHCI/console/storage.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use alloc::vec::Vec;
use spin::Mutex;

use crate::arch::{acpi, apic, gdt, interrupts, percpu, sched, syscall};

/// Physical page the trampoline is copied to and the AP starts executing at. Must be page-aligned
/// and < 1 MiB (the SIPI vector is 8 bits: start address = vector << 12). 0x8000 is free
/// conventional RAM in our UEFI memory map once boot services have exited.
const TRAMPOLINE_ADDR: usize = 0x8000;
/// SIPI vector byte that selects `TRAMPOLINE_ADDR` (0x8000 >> 12 = 0x08).
const SIPI_VECTOR: u8 = (TRAMPOLINE_ADDR >> 12) as u8;

/// Per-AP kernel stack size (the BSP keeps its UEFI boot stack; only APs need new ones).
const AP_STACK_SIZE: usize = 4096 * 4; // 16 KiB

/// 16-byte-aligned AP kernel stacks in `.bss`, one per logical CPU (index 0 / the BSP is unused).
/// Static, not heap: APs touch no shared allocator state during bring-up.
#[repr(C, align(16))]
struct ApStack([u8; AP_STACK_SIZE]);
static mut AP_STACKS: [ApStack; gdt::MAX_CPUS] =
    [const { ApStack([0; AP_STACK_SIZE]) }; gdt::MAX_CPUS];

/// Count of APs that have reached `ap_entry` and finished their own bring-up. The BSP waits on
/// this between SIPIs so the shared trampoline handoff slot (stack/index) is reused safely.
static AP_ONLINE: AtomicU32 = AtomicU32::new(0);

/// Logical indices of the APs that came online, published by `start_aps` for the scheduler to
/// spawn work onto. Written once on the BSP after bring-up; read on the BSP.
static ONLINE_APS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// Snapshot of the online application-processor logical indices (excludes the BSP).
pub fn online_aps() -> Vec<usize> {
    ONLINE_APS.lock().clone()
}

// ── WITCORE: SCHED-X86 core placement ───────────────────────────────────────────────────────────
//
// Until this arc, "which core does this work go on?" was answered independently at a dozen call
// sites, and every one of them answered `online_aps().first()` / `.get(1)` / `.get(2)`. That was
// correct while the APs were an undifferentiated pool. SCHED-X86 (`a571254f`) ended that: it pinned
// `x86_render_service` to `online_aps().first()` and `x86_usb_pump` + `x86_input_service` to
// `online_aps().last()`. Every unrelated site that still said `.first()` therefore started aiming at
// the RENDER core by coincidence, including:
//
//   * the ring-3 fixture ladder (`u2/u4x/u5x/u6x/u6bx/u7x/u6gx_probe_once`), which places
//     COOPERATIVE (IF=0) ring-3 tasks — a task that owns its core until it makes a syscall. On the
//     render core that is a stalled panel at best;
//   * `irqstorage::start_service_once`, which places the PREEMPTIBLE `storage-svc` task — and
//     `service_one` -> `block::read_block` takes the raw `XHCI_CONTROLLER` spin lock, which the
//     render/shell task also takes (FAT reads, `pal::pump_and_poll`, `usbinfo`). Two preemptible
//     takers of a raw spinlock on ONE core is the hard deadlock SCHED-X86's own rule 1 forbids.
//
// So placement is stated HERE, once, and asked for by name. Two rules, two functions:
//
//   `worker_cpu(n)`      — work that must not share the render core (cooperative ring 3, launchers).
//   `xhci_worker_cpu(n)` — additionally must not share the SERVICE core, because it is a preemptible
//                          taker of `XHCI_CONTROLLER` and so is `x86_usb_pump`. Returns `None`
//                          rather than degrade: co-locating is a deadlock, and declining is the
//                          honest fallback (the same choice the handoff itself makes).
//
// BEFORE the handoff publishes a split — and on every build that never takes it (`rast`, `usbdebug`,
// a single-AP box, the pre-GUI `witness` fixture block) — both helpers degrade to indexing
// `online_aps()` directly. That reproduces the pre-WITCORE placement at twelve of the thirteen
// converted sites. It is NOT true at the thirteenth: `irqstorage::selftest_once` deliberately lost
// its `.get(1).or_else(|| .first())` fallback, because the two takers that fallback co-located are
// both preemptible holders of `XHCI_CONTROLLER`. On a 1-AP `usbdebug`/`rast`/no-split build
// `bx-blockreq` therefore no longer runs; it prints a SKIP line and `selftest_verdict()` reads 0.
// That is a real behaviour change, chosen over a real deadlock, and it is called out here rather
// than hidden behind "byte-identical".

/// Sentinel: no SCHED-X86 split has been published yet.
const NO_SPLIT: u64 = u64::MAX;

/// The published SCHED-X86 split, as ONE word: `(render << 32) | service`.
///
/// One word deliberately. Two `AtomicUsize`s written with two `Release` stores are read with two
/// `Acquire` loads, and a reader landing between them sees `(Some(render), None)` — which
/// `worker_pool` classifies as `Unpublished`, which hands out `online_aps()[0]`, which IS the render
/// core. That is the exact defect this module removes, resurrected on some boots only. Today the
/// publisher runs before any reader can exist, so the window is unreachable — but that is a property
/// of statement order in `kernel_main`, not of the data, and the next arc to move a spawn earlier
/// would not be told. A single word makes the pair unobservably-partial by type.
static SPLIT: AtomicU64 = AtomicU64::new(NO_SPLIT);

/// The published split as a pair, or `None` before `publish_sched_split`. ONE load — every caller
/// that needs both halves goes through this rather than two independent reads.
fn split() -> Option<(usize, usize)> {
    let v = SPLIT.load(Ordering::Acquire);
    (v != NO_SPLIT).then(|| ((v >> 32) as usize, (v & 0xffff_ffff) as usize))
}

/// The render service's core, once the SCHED-X86 handoff has published it.
pub fn render_cpu() -> Option<usize> {
    split().map(|(r, _)| r)
}

/// The device-service core (`x86_usb_pump` / `x86_input_service`), once published.
pub fn service_cpu() -> Option<usize> {
    split().map(|(_, s)| s)
}

/// How much room the worker pool actually had — reported in the placement witness so the log says
/// which rule was satisfiable rather than leaving it to be inferred.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PlaceTier {
    /// No split published: the helpers index `online_aps()` directly (pre-handoff / inline-GUI builds).
    Unpublished,
    /// Cores exist that are neither the render core nor the service core. The only tier in which
    /// `xhci_worker_cpu` will hand anything out.
    Exclusive,
    /// Only the service core is left after excluding render. Workers share with the pump; xHCI
    /// takers are declined.
    SvcShared,
    /// Nothing to exclude (single AP). Unreachable post-publish — the split needs 2 distinct cores.
    RenderShared,
}

impl PlaceTier {
    pub fn name(self) -> &'static str {
        match self {
            PlaceTier::Unpublished => "unpublished",
            PlaceTier::Exclusive => "exclusive",
            PlaceTier::SvcShared => "svc-shared",
            PlaceTier::RenderShared => "render-shared",
        }
    }
}

/// The eligible cores for non-render work, in placement order, plus the tier that produced them.
fn worker_pool() -> (Vec<usize>, PlaceTier) {
    let online = online_aps();
    let (Some(r), Some(s)) = (render_cpu(), service_cpu()) else {
        return (online, PlaceTier::Unpublished);
    };
    let excl: Vec<usize> = online.iter().copied().filter(|&c| c != r && c != s).collect();
    if !excl.is_empty() {
        return (excl, PlaceTier::Exclusive);
    }
    let no_render: Vec<usize> = online.iter().copied().filter(|&c| c != r).collect();
    if !no_render.is_empty() {
        return (no_render, PlaceTier::SvcShared);
    }
    (online, PlaceTier::RenderShared)
}

/// The `n`th core (0-based) for work that MUST NOT share a core with `x86_render_service`.
///
/// STRICT indexing: `None` when the pool is shorter than `n + 1`. Callers that need k mutually
/// distinct cores (the cooperative ring-3 choreographies — U7x, SOCK-4, U6gx — where a polling
/// fixture hogs its core) depend on that: a silent fallback to a shared core would deadlock the
/// GO/SIG sequencing, which is strictly worse than the clean skip they already print.
pub fn worker_cpu(nth: usize) -> Option<usize> {
    worker_pool().0.get(nth).copied()
}

/// The `n`th core for a PREEMPTIBLE task that takes `XHCI_CONTROLLER` (a raw `spin::Mutex`).
///
/// Excludes BOTH the render core and the service core: `x86_usb_pump` holds that lock on the service
/// core and the render/shell task holds it on the render core, and two preemptible takers of a raw
/// spinlock on one core deadlock it (preempt the holder; the spinner cannot yield). `None` when no
/// such core exists — the caller must DECLINE, never co-locate.
pub fn xhci_worker_cpu(nth: usize) -> Option<usize> {
    let (pool, tier) = worker_pool();
    match tier {
        PlaceTier::Exclusive | PlaceTier::Unpublished => pool.get(nth).copied(),
        PlaceTier::SvcShared | PlaceTier::RenderShared => None,
    }
}

/// Formats an optional core index as `c3` / `-` for the placement witness.
struct CpuOpt(Option<usize>);

impl core::fmt::Display for CpuOpt {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0 {
            Some(c) => write!(f, "c{}", c),
            None => write!(f, "-"),
        }
    }
}

/// Publish the SCHED-X86 split and emit the placement witness.
///
/// Called ONCE by the handoff in `kernel_main`, BEFORE the three service tasks are spawned, so every
/// later `worker_cpu` / `xhci_worker_cpu` question is answered against the real map.
///
/// This line carries the MAP and nothing else. It deliberately carries no PASS/FAIL.
///
/// The first version of it printed `render-clear` and `xhci-clear`, and both were tautologies:
/// `render-clear` re-applied the very filter that had just built the pool (and `render != service`
/// is guaranteed by the match guard in `kernel_main` that is the only path here), and `xhci-clear`'s
/// FAIL arm was unreachable code. They printed PASS on every boot that printed the line at all —
/// including captures where `bx-blockreq` had been handed no core. A verdict that cannot fail is
/// worse than no verdict: it reads as evidence and is not. The real verdict is
/// [`confirm_render_core`], which fires later, from the render task itself, against what the
/// SCHEDULER reports rather than against the arguments this function was handed.
///
/// Reading the line: `worker[0..2]` are the cores the ring-3 fixture ladder will take (U2/U4x/U5x/
/// U6x/U6bx/U7x/U6gx and their launchers/verdict tasks); `xhci[0]` is `irqstorage`'s `storage-svc`
/// and `xhci[1]` is its `bx-blockreq` self-test. A `-` means that consumer got NO core and will skip
/// — the short-pool signal, printed rather than rounded up to PASS.
pub fn publish_sched_split(render: usize, service: usize) {
    SPLIT.store(((render as u64) << 32) | (service as u64 & 0xffff_ffff), Ordering::Release);

    let (pool, tier) = worker_pool();
    let w = [
        CpuOpt(pool.first().copied()),
        CpuOpt(pool.get(1).copied()),
        CpuOpt(pool.get(2).copied()),
    ];
    let x = [CpuOpt(xhci_worker_cpu(0)), CpuOpt(xhci_worker_cpu(1))];

    serial_println!(
        ":: SCHED-X86 PLACE: aps={} render=c{} svc=c{} worker=[{},{},{}] xhci=[{},{}] tier={} pool={} ::",
        online_aps().len(),
        render,
        service,
        w[0],
        w[1],
        w[2],
        x[0],
        x[1],
        tier.name(),
        pool.len(),
    );
}

/// The placement VERDICT — called by `x86_render_service` itself, on the core the scheduler actually
/// dispatched it to, right after its own dispatch witness.
///
/// This is the check [`publish_sched_split`] could not honestly make. It compares THREE values with
/// three independent producers, none of which is the argument the publisher was handed:
///
///   1. `actual` — `percpu::this_cpu().cpu_index`, i.e. the core the hardware says this code is
///      executing on, read here rather than accepted from a caller. Produced by GS/per-CPU setup.
///   2. `arg` — the core the SPAWN SITE asked for, carried through the task's argument. Produced by
///      `kernel_main`.
///   3. the split read BACK out of `SPLIT`, and the pool `worker_cpu`/`xhci_worker_cpu` will hand
///      out, re-derived now. Produced by this module.
///
/// It can therefore fail for real: a `spawn` that ignored its pin, a steal that moved a task marked
/// pinned, a second publisher, a torn `SPLIT`, or a pool that contains the core the renderer
/// actually landed on would each show up. Verdicts:
///
///   `PASS`    — all three agree, and no core this module hands out equals the running renderer's.
///   `PARTIAL` — the above holds, but some consumer class got no core (`xhci=[-,…]` on the PLACE
///               line): the rule is intact, coverage is not. Its own word, because rounding this up
///               to PASS is exactly what made the first version of this witness useless.
///   `FAIL`    — a placement collides with the running renderer, or the core it is running on is not
///               the one that was published/requested.
pub fn confirm_render_core(arg: usize) {
    let actual = percpu::this_cpu().cpu_index as usize;
    let published = split().map(|(r, _)| r);
    let (pool, tier) = worker_pool();
    let collide = pool.iter().filter(|&&c| c == actual).count();
    let agree = published == Some(actual) && arg == actual;
    let short = xhci_worker_cpu(0).is_none() || xhci_worker_cpu(1).is_none();

    let verdict = if !agree || collide > 0 {
        "FAIL"
    } else if short {
        "PARTIAL"
    } else {
        "PASS"
    };

    serial_println!(
        ":: SCHED-X86 PLACE-CHECK: actual=c{} arg=c{} published={} pool={} collide={} tier={} verdict={} ::",
        actual,
        arg,
        CpuOpt(published),
        pool.len(),
        collide,
        tier.name(),
        verdict,
    );
}

// The real-mode -> long-mode trampoline. AT&T syntax; see the module comment for the design.
// Every absolute reference is `TRAMP + (label - ap_trampoline_start)` so the assembled bytes
// carry no relocations and are valid only after being copied to TRAMP (0x8000).
core::arch::global_asm!(
    r#"
.section .rodata
.balign 16
.code16
.global ap_trampoline_start
ap_trampoline_start:
    cli
    cld
    xorw   %ax, %ax
    movw   %ax, %ds
    movw   %ax, %es
    movw   %ax, %ss
    # Load the temporary GDT (absolute address, ds = 0).
    lgdtl  0x8000 + ap_gdt_ptr - ap_trampoline_start
    # Enter protected mode.
    movl   %cr0, %eax
    orl    $1, %eax
    movl   %eax, %cr0
    # Far jump into the 32-bit code segment (selector 0x08).
    ljmpl  $0x08, $(0x8000 + ap_pm_entry - ap_trampoline_start)

.code32
ap_pm_entry:
    movw   $0x10, %ax            # 32-bit data selector
    movw   %ax, %ds
    movw   %ax, %es
    movw   %ax, %ss
    movw   %ax, %fs
    movw   %ax, %gs
    # Enable PAE (CR4.PAE, bit 5) — required for long mode.
    movl   %cr4, %eax
    orl    $0x20, %eax
    movl   %eax, %cr4
    # Load the BSP's PML4 (shared identity map) into CR3.
    movl   $(0x8000 + ap_param_cr3 - ap_trampoline_start), %ecx
    movl   (%ecx), %eax
    movl   %eax, %cr3
    # Set EFER.LME (long mode enable, bit 8).
    movl   $0xC0000080, %ecx
    rdmsr
    orl    $0x100, %eax
    wrmsr
    # Enable paging + protection (CR0.PG | CR0.PE) — activates long mode.
    movl   %cr0, %eax
    orl    $0x80000001, %eax
    movl   %eax, %cr0
    # Far jump into the 64-bit code segment (selector 0x18).
    ljmpl  $0x18, $(0x8000 + ap_lm_entry - ap_trampoline_start)

.code64
ap_lm_entry:
    xorl   %eax, %eax
    movw   %ax, %ds
    movw   %ax, %es
    movw   %ax, %ss
    movw   %ax, %fs
    movw   %ax, %gs
    # rsp = *(param_stack); rdi = *(param_index); jump to *(param_entry).
    movl   $(0x8000 + ap_param_stack - ap_trampoline_start), %ecx
    movq   (%rcx), %rsp
    movl   $(0x8000 + ap_param_index - ap_trampoline_start), %ecx
    movq   (%rcx), %rdi
    movl   $(0x8000 + ap_param_entry - ap_trampoline_start), %ecx
    movq   (%rcx), %rax
    jmpq   *%rax

# --- Temporary GDT: null, 32-bit code (0x08), 32-bit data (0x10), 64-bit code (0x18). ---
.balign 8
ap_gdt:
    .quad 0x0000000000000000
    .quad 0x00CF9A000000FFFF
    .quad 0x00CF92000000FFFF
    .quad 0x00AF9A000000FFFF
ap_gdt_ptr:
    .word ap_gdt_ptr - ap_gdt - 1
    .long 0x8000 + ap_gdt - ap_trampoline_start

# --- Parameter block, patched by the BSP before each SIPI. ---
.balign 8
.global ap_param_cr3
.global ap_param_entry
.global ap_param_stack
.global ap_param_index
ap_param_cr3:    .quad 0
ap_param_entry:  .quad 0
ap_param_stack:  .quad 0
ap_param_index:  .quad 0
.global ap_trampoline_end
ap_trampoline_end:
"#,
    options(att_syntax)
);

unsafe extern "C" {
    static ap_trampoline_start: u8;
    static ap_trampoline_end: u8;
    static ap_param_cr3: u8;
    static ap_param_entry: u8;
    static ap_param_stack: u8;
    static ap_param_index: u8;
}

/// 64-bit entry for a freshly long-moded AP. Runs on this AP's own stack with `cpu_index` passed
/// in rdi by the trampoline. Brings the AP fully online: its own per-CPU GDT/TSS, the shared IDT,
/// and its own local APIC (x2APIC + timer), then idles. Never touches xHCI/console/heap — those
/// stay BSP-owned.
#[unsafe(no_mangle)]
pub extern "C" fn ap_entry(cpu_index: u64) -> ! {
    let idx = cpu_index as usize;
    gdt::init_cpu(idx);
    interrupts::init_idt();
    apic::init();
    // SCHED-X86 / PAT-WC: program THIS core's IA32_PAT so slot 4 == Write-Combining, exactly as
    // `arch::memory` prescribes for an AP that blits (see the `ensure_pat_wc` doc block). The PAT is
    // PER-CORE MSR state; the framebuffer's LEAF retype is not (APs run on the BSP's CR3, so the
    // `set_framebuffer_wc` retype is already visible here). Without this line an AP's fb PTE selects
    // PA4 = its unmodified WB default, which under the firmware's UC var-range MTRR is EFFECTIVE-UC:
    // every blit on that core would be an uncombined UC store train. The render service is now a
    // scheduled task pinned to an AP, so that core IS a blitting core — but we program EVERY core
    // rather than the chosen one, so the placement decision is not load-bearing and a later re-pin
    // cannot silently regress the panel. PAT=WC wins over any MTRR type (SDM Table 11-7; the tree's
    // own decode is `video::vperf`'s `1 => 1, // PAT WC: WC regardless of MTRR`), so this is correct
    // whatever firmware left in the APs' MTRRs.
    //
    // Legal HERE, this early: `ensure_pat_wc` is CPUID + RDMSR/WRMSR only — no GDT, no per-CPU GS, no
    // heap, no interrupts. Placed BEFORE the `AP_ONLINE` handshake and before `sti`, so no AP is ever
    // advertised online — or takes an interrupt — without PA4=WC programmed.
    crate::arch::memory::ensure_pat_wc();

    let apic_id = apic::apic_id_u32();
    // Per-CPU data + GS base before enabling interrupts, so this AP's timer/IPI handlers can
    // resolve `this_cpu()`.
    percpu::init_cpu(idx, apic_id);
    // U1a: this AP's SYSCALL/SYSRET MSRs + NX/SMEP (after its GDT + per-CPU data, before `sti`), so
    // a ring-3 task dispatched onto this AP can trap back in.
    syscall::init();

    AP_ONLINE.fetch_add(1, Ordering::SeqCst);
    serial_println!("SMP: AP {} online (apic id {}).", idx, apic_id);

    x86_64::instructions::interrupts::enable();
    // Wait until the BSP has run SMP verification and turned scheduling on, then run this AP's
    // scheduler loop forever (replacing the old idle `hlt_loop`). The BSP keeps driving
    // xHCI/console/storage; APs run scheduled kernel threads.
    sched::wait_and_run();
}

/// Patch one 8-byte field of the (already-copied) trampoline parameter block at TRAMPOLINE_ADDR.
/// `param` is the link-time symbol; its offset from `ap_trampoline_start` is the same at 0x8000.
unsafe fn patch_param(param: *const u8, val: u64) {
    let start = &raw const ap_trampoline_start as usize;
    let off = param as usize - start;
    unsafe { core::ptr::write_volatile((TRAMPOLINE_ADDR + off) as *mut u64, val) };
}

/// Crude bounded busy-wait. We don't have a calibrated microsecond clock yet (the APIC timer is
/// uncalibrated), but exact timing isn't needed: the INIT/SIPI delays only have to let the AP
/// latch each command, and the real synchronisation is the `AP_ONLINE` handshake below.
fn spin_delay(iterations: u64) {
    for _ in 0..iterations {
        core::hint::spin_loop();
    }
}

/// Architectural INIT-SIPI-SIPI to start the AP with the given APIC id.
fn init_sipi_sipi(apic_id: u32) {
    const INIT: u32 = 0x0000_4500; // delivery mode 101 (INIT), level assert
    let sipi: u32 = 0x0000_4600 | SIPI_VECTOR as u32; // delivery mode 110 (Startup), assert, vector

    apic::send_ipi(apic_id, INIT);
    spin_delay(2_000_000); // ~INIT settle (>=10ms on real HW; generous here)
    apic::send_ipi(apic_id, sipi);
    spin_delay(100_000); // ~200us between SIPIs
    apic::send_ipi(apic_id, sipi);
}

/// Bring up every application processor reported by ACPI. Called once on the BSP after ACPI
/// discovery. APs are started one at a time (the trampoline's stack/index handoff slot is shared)
/// and the BSP waits for each to report online before starting the next. Degrades cleanly:
/// missing topology, or an AP that never checks in, just leaves that core offline.
pub fn start_aps() {
    let topo = match acpi::topology() {
        Some(t) => t,
        None => {
            serial_println!("SMP: no ACPI topology; staying uniprocessor.");
            return;
        }
    };
    let apic_ids = topo.apic_ids();
    if apic_ids.len() <= 1 {
        serial_println!("SMP: 1 CPU; no APs to start.");
        return;
    }

    let bsp_id = apic::apic_id_u32();

    // Validate the fixed trampoline page against the real UEFI map. 0x8000 is free conventional RAM
    // on QEMU/OVMF, but Apple EFI fragments low memory and may mark it Reserved/Bootloader — in
    // which case writing the trampoline there (or the AP executing it) is unsound and APs may
    // silently fail to start. This turns that into a visible breadcrumb on the (serial-less) Mac;
    // the fix if it fires is to scan the map for a free low page and retarget the SIPI vector.
    if !crate::arch::memory::region_is_usable(TRAMPOLINE_ADDR as u64, 0x1000) {
        serial_println!(
            "SMP: WARNING: trampoline page {:#x} is NOT Usable in the UEFI map — APs may fail to \
             start (firmware may reclaim/clobber it).",
            TRAMPOLINE_ADDR
        );
    }

    // Copy the trampoline to its low page and patch the fields common to every AP.
    let cr3 = x86_64::registers::control::Cr3::read().0.start_address().as_u64();
    unsafe {
        let start = &raw const ap_trampoline_start as *const u8;
        let end = &raw const ap_trampoline_end as *const u8;
        let len = end as usize - start as usize;
        core::ptr::copy_nonoverlapping(start, TRAMPOLINE_ADDR as *mut u8, len);
        patch_param(&raw const ap_param_cr3, cr3);
        patch_param(&raw const ap_param_entry, ap_entry as *const () as u64);
    }
    serial_println!(
        "SMP: starting APs (trampoline @ {:#x}, cr3 {:#x}, {} CPUs)...",
        TRAMPOLINE_ADDR,
        cr3,
        apic_ids.len()
    );

    // Logical indices of APs that reported online (BSP is 0, handled separately).
    let mut online_aps: [usize; gdt::MAX_CPUS] = [0; gdt::MAX_CPUS];
    let mut n_online = 0usize;

    let mut index = 1usize; // logical CPU 0 is the BSP
    for &id in apic_ids {
        if id == bsp_id {
            continue;
        }
        if index >= gdt::MAX_CPUS {
            serial_println!("SMP: MAX_CPUS reached; skipping remaining APs.");
            break;
        }

        // Per-AP handoff: 16-byte-aligned stack top minus 8 (SysV ABI expects rsp%16==8 at a
        // function entry reached via call; we arrive via jmp, so bias by 8) and the logical index.
        let stack_top = unsafe {
            let base = &raw const AP_STACKS[index] as usize;
            (base + AP_STACK_SIZE - 8) as u64
        };
        unsafe {
            patch_param(&raw const ap_param_stack, stack_top);
            patch_param(&raw const ap_param_index, index as u64);
        }

        let target = AP_ONLINE.load(Ordering::SeqCst) + 1;
        init_sipi_sipi(id);

        // Wait (bounded) for this AP to report in before reusing the handoff slot.
        let mut came_online = false;
        for _ in 0..50_000_000u64 {
            if AP_ONLINE.load(Ordering::SeqCst) >= target {
                came_online = true;
                break;
            }
            core::hint::spin_loop();
        }
        if came_online {
            online_aps[n_online] = index;
            n_online += 1;
        } else {
            serial_println!("SMP: WARNING: AP apic id {} did not come online (timeout).", id);
        }

        index += 1;
    }

    serial_println!(
        "SMP: bring-up complete — {} of {} CPUs online (incl. BSP).",
        AP_ONLINE.load(Ordering::SeqCst) + 1,
        apic_ids.len()
    );

    // Publish the online AP indices so the scheduler can spawn work onto exactly the cores that
    // actually came up (not just "1..cpu_count").
    *ONLINE_APS.lock() = online_aps[..n_online].to_vec();

    verify_smp(&online_aps[..n_online]);
}

/// Post-bring-up smoke test: prove the SMP plumbing actually works. Confirms every core's local
/// APIC timer is ticking (each CPU has its own per-CPU tick counter) and that each AP answers a
/// fixed IPI (the cross-CPU wakeup primitive a scheduler will use).
fn verify_smp(online_aps: &[usize]) {
    // Let real time pass by waiting for the BSP's own timer to advance several ticks, so the APs'
    // (earlier-armed) timers have certainly ticked too.
    let bsp_ticks = percpu::this_cpu();
    let base = bsp_ticks.ticks.load(Ordering::Relaxed);
    for _ in 0..200_000_000u64 {
        if bsp_ticks.ticks.load(Ordering::Relaxed) >= base + 5 {
            break;
        }
        core::hint::spin_loop();
    }

    // Per-CPU timer: BSP first, then each online AP.
    serial_println!(
        "SMP: per-CPU timer — cpu 0 (apic {}) ticks={}",
        bsp_ticks.apic_id,
        bsp_ticks.ticks.load(Ordering::Relaxed)
    );
    for &i in online_aps {
        if let Some(c) = percpu::cpu(i) {
            serial_println!(
                "SMP: per-CPU timer — cpu {} (apic {}) ticks={}",
                i,
                c.apic_id,
                c.ticks.load(Ordering::Relaxed)
            );
        }
    }

    // IPI round-trip: knock each AP with a fixed IPI and confirm its handler ran.
    let icr_low = 0x0000_4000 | interrupts::IPI_VECTOR as u32; // fixed delivery, assert, vector
    for &i in online_aps {
        let Some(c) = percpu::cpu(i) else { continue };
        let before = c.ipis.load(Ordering::SeqCst);
        apic::send_ipi(c.apic_id, icr_low);

        let mut acked = false;
        for _ in 0..20_000_000u64 {
            if c.ipis.load(Ordering::SeqCst) > before {
                acked = true;
                break;
            }
            core::hint::spin_loop();
        }
        serial_println!(
            "SMP: IPI -> cpu {} (apic {}): {}",
            i,
            c.apic_id,
            if acked { "ack" } else { "NO ACK" }
        );
    }
}
