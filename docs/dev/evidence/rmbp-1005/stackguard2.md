# STACKGUARD2 (rmbp-ledger B403) — every x86 kernel stack guarded, the shootdown an IPI, a dead task's locks released

## Finding (the wire, then the code)
- Flight 24 card 3 reboot: `[stack] OVERFLOW task=play-dec stack=0x20d43968..0x20d53968 fault=0x20d42c08 … via=df -> task halted`
  — SMALLFIX M6's guard caught the first overflow ever (the AAC constructor chain, DECJOBHANG B386). It went `via=df`:
  the #PF could not push its frame on the exhausted task stack, so the #DF IST carried the whole recovery.
- Not guarded on this tree: the AP boot stacks (`smp.rs` `AP_STACKS`, 16 KiB `.bss`, align 16, contiguous — an AP
  overflow writes the previous AP's stack), the BSP's firmware stack (UEFI BootServicesData, `Reserved` in our map,
  bounds unknown to the kernel), and the 4×MAX_CPUS IST stacks (`gdt.rs` `IST_STACKS`, 8 KiB each, contiguous).
- The remote flush after an unmap is `stackguard::tlb_sync` in the timer tick (a generation compare): a core can keep
  a stale 2 MiB/1 GiB leaf over a fresh guard for up to one tick — and CLOCKCORE (B397) moves the tick.
- A task halted by an overflow keeps every lock it held. Flight 25's FLAC wedge left the wire SILENT; on the rMBP the
  wire IS the FTDI capture ring (`drivers/xhci/ftdi.rs` `RING`, taken with `try_lock`, held while a line is
  formatted, IF possibly on). A task that dies inside that hold leaves `RING` locked forever: every later line is
  staged, never drained — the OVERFLOW line itself included. The UnaFS mount lock (`fs/unafs.rs` `MOUNT`, held
  IF=0 across a whole transaction) is the other lock a decoder reading `/system/test-f` holds for long spans.
  `overflow_exit` today waits 20M spins on `SERIAL1` and then flips the WHOLE machine into panic-mode serial.

## The DF IST — reasoned before a line was written
A guard under the #DF IST cannot triple-fault: #DF delivery always loads RSP from the TSS (the IST TOP, mapped), so the
only way to fault while delivering #DF is an unmapped IST top, which this arc never makes. What a guard under it CAN
do is turn a too-deep #DF handler into a #PF-in-#DF -> #DF re-delivery at the IST top, clobbering the running
handler's frames — a loop, not a triple fault. Two answers: (1) size: the #DF IST goes 8 KiB -> 32 KiB usable. A task
overflow leaves the IST within the first ~200 bytes (`on_kernel_fault` switches RSP to the dead task's slab top
before calling anything); only a FATAL #DF (a static-stack overflow, or any other kernel #DF) runs the panic path
(`panic_screen` + fbcon + ftdi + the formatter) on it, and that path runs today on the 32 KiB render task; (2) a
per-core #DF depth: a #DF that arrives while this core is already in the #DF path writes one lock-free line
(`[stack] OVERFLOW stack=ist-df … -> core halted`) and `hlt`s with IF=0 — it never re-enters the panic path.
The NMI/#DB/#MC ISTs keep 8 KiB usable (their handlers are leaf, GS-free). `tests stackroom` reads the #DF IST's high
mark, so the 32 KiB is measured, not asserted.

## Seam
Kernel — driver (scheduler + paging + `arch/x86_64` stacks; no handler's domain). The lock-owner registry is a new
arch-neutral `lockowner.rs` (CHARTER: Kernel — kernel-by-ruling): a tracked lock records its holder's task id while
held; nothing else changes about how the lock is taken.

## Milestones
- M1 static guards: `ApStack`/IST stacks become page-aligned with a guard page under each (usable sizes unchanged
  except the #DF IST 8 -> 32 KiB); `stackguard::arm_cpu(i)` unmaps an AP's stack guard and its four IST guards from
  the BSP BEFORE the SIPI (one hook line in `smp::start_aps`); cpu 0's IST guards and the BSP stack guard (the
  floor page of the UEFI descriptor holding `_start`'s RSP, armed only when that descriptor is `Reserved` and at
  least 128 KiB — the UEFI minimum — lies between its floor and the top; otherwise the line says `bsp=owed`).
  The fault side names `stack=<task|bsp|ap<n>|ist-<df|nmi|db|mc><cpu>>`; a static-stack overflow is fatal by
  nature (an idle/scheduler or handler context has no task to halt): one OVERFLOW line, then the panic, on a fresh
  #DF IST frame. Boot line `[stack] guards armed tasks=<n> aps=<n> ist=<n> page=4096`.
- M2 the shootdown is an IPI: vector `tlb` from the allocator; after every unmap the arming core sends a fixed IPI
  to every core that has ticked (online) and waits for each core's ack generation, bounded at 2 ms, acking any
  concurrent shootdown itself while it waits (no deadlock between two IF=0 senders). The tick's `tlb_sync` stays as
  the backstop for a core that was masked past the bound. `[tlb] shootdown pages=1 cpus=<n> acked=<n> us=<n>` the
  first time, then only on a timeout. Independent of the tick (CLOCKCORE B397): it needs only the LAPIC ICR.
- M3 a halted task releases what it is KNOWN to hold: `lockowner` tracks the FTDI capture ring (`sink`), the 16550
  `SERIAL1` (`uart`) and the UnaFS mount (`unafs`); `overflow_exit` force-unlocks each lock whose holder is the dead
  task, BEFORE its line (so the line reaches the wire), and the UnaFS mount instance is LEAKED, not dropped (its Drop
  would write back a half-done transaction's metadata; CoW: the medium is consistent at the last root flip, the next
  call re-binds). `[stack] overflow task=<t> released=[sink,unafs]`. The 20M-spin + machine-wide panic mode go.
- M4 `tests stackroom` covers every kind (task, bsp, ap, ist) with its high mark (static stacks painted at arm time);
  `tests stackroom overflow` (test-only, R80: never at boot) spawns a scratch task on a worker core that recurses
  off its 16 KiB stack, waits for its OVERFLOW, and reads that core's #DF IST high mark.
- M5 witness `:: STACKROOM: tasks=<n> aps=<n> ist=<n> armed=<n> live=<n> failed=0 ipi_flush=ok -> PASS ::`.

## Witness lines (what the next flight reads)
- boot: `[stack] guards armed tasks=<n> aps=<n> ist=<n> bsp=<armed|owed> page=4096`, `[tlb] shootdown pages=1 cpus=<n> acked=<n> us=<n>`
- `tests stackroom`: `[stack] room kind=<task|bsp|ap|ist-…> …`, `:: STACKROOM: … ipi_flush=ok -> PASS ::`
- `tests stackroom overflow`: `[stack] OVERFLOW task=sg-scratch … via=<df|pf> -> task halted`,
  `[stack] overflow task=sg-scratch released=[]`, `:: STACKOVF: task=sg-scratch cpu=<c> via=<df|pf> halted=1 df_ist_high=<n> of 32768 -> PASS ::`

## Owed
- fbcon's console lock and every other lock (no general lock registry exists: `arch::sync` is absent on this tree).
- A core masked (IF=0) past the 2 ms bound sees a new guard at its next tick or IPI after it unmasks.
- The BSP guard is the floor of the firmware's descriptor, not a measured stack floor (the firmware does not say).
