# WINDOWCAP3 (rmbp-ledger B399, R90) — design

**Finding (flights 24/25, every boot):** `[wm] limit windows=148 procs=10 from=mem:189,asids:10 (R90)` —
the process limit is pinned by `asids:10`, i.e. `USER_SLOTS = 12` (x86) / 8 (aarch64) less a 2-slot
reserve: a static `.bss` pool of page tables + 1.3 MiB backing per slot. `FB_WIN_SLOTS = 4` windows per
process and every `[_; USER_SLOTS]` sidecar are the same constant.

**Seam:** Kernel — kernel-by-ruling (R90). No second store: the slot pool stays in `arch::*::memory`/
`boot`/`mmu_tegra_el0` and the sidecars stay in `syscall.rs`; only their storage changes.

* `procslot.rs` (new): `SlotVec<T>` — a `rowstore::SegVec` keyed by address-space slot plus the one
  SHARED row, `Index`-able so `X[s]` call sites keep their shape. The bound on a slot index is its TYPE:
  the futex key tag byte (`(slot + 1) << 56`, bit 63 reserved) — `SLOT_ID_MAX = 127`, the WinId rule.
* Pool: a slot's page tables + args page + backing are ONE heap record (`alloc_zeroed`, 4 KiB-aligned,
  identity-mapped heap = PA), allocated on the first claim of that index and RECYCLED by index (the
  tegra model). Freed-to-heap on exit is NOT done: a core can hold a dead slot's CR3 loaded until its
  next dispatch, so returning the PML4 frame to the heap needs a cross-core CR3 quiescence first (owed).
* `procs = mem` only: `from=mem:<MiB>` — `asids` leaves the `[wm] limit` line; PROC_COST now carries
  the slot record (backing + tables) so the memory term is honest.
* Per-process windows: the region-slot range is unbounded up to the info page's own row count
  (`(4096 - 0x40) / 0x20 = 126`, the info page's TYPE); region slots 0..=3 keep their ABI VAs, 4.. get
  their own heap frames in a VA band above the ELF window (the info-page entry's offset field carries
  the VA). WIN_MAX and the ring-3 window table become a growable Vec.

**Milestones (as built):** M1+M2 `procslot::SlotVec` (a `SegVec` keyed by slot, the x86 shared row
inline at a sentinel, aarch64's ASID 0 inline) + the x86 pool as one heap record per slot (`SlotMem`:
PML4/PDPT/PD/PT + args page + backing) + every x86 per-slot sidecar, the process table (`PROCS`) and the
ring-3 window table (`WINDOWS`) grown · M4 `wincap`: `procs = min(mem, windows)` (PROC_COST now carries
the slot record), `asids` gone from the wire, the kernel launchers ask the valve (`proc_admit`), `tests
spawnstorm` · M5 aarch64: Pi/tegra slot tables (+ Pi backing) heap records, xwin extension records, the
ASID-keyed sidecars, `PROCS`/`KILLS`/windows grown, the 64-bit DETACHED/HIDDEN ASID words -> `SlotBits`,
`SLOT_CORE_RES`/`ASID_THREADS` (was `[_; 9]`), every launch asks the valve · M6 x86 windows per process
past the record's four: region slot >= 4 gets a heap surface in a band above the ELF window
(`USER_BASE + XWIN_OFF + XWIN_BYTES`), its offset in the info-page entry; bound = the info page's rows
(126, its TYPE).

**Witness:** `[wm] limit windows=<n> procs=<n> from=mem:<MiB> (R90)` at arming;
`:: WINDOWCAP3: fixed_pools=none procs_limit=<n> from=mem spawned=<n> refused_at=<n> paused_ms=<n> machine_alive=1 -> PASS ::`
(+ `base= reaped= slots= heap_free_mib=a->b refusal=`) from `tests spawnstorm` (x86).

**Owed:** slot records returned to the heap on exit (needs a cross-core CR3/TTBR quiescence proof);
aarch64 windows per process past the four (the band would live in xwin's extension GiB); a
`tests spawnstorm` twin on aarch64; a 5th-window fixture on x86; stack and heap use measured on the
next flight.
