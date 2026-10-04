# RING3WIN — a UnaOS ring-3 program gets the address space its ELF asks for (B316)

CHARTER: Kernel — driver (Zone 2, the Map). Branch `exec-rmbp-ring3win`, cut from f7a0d15e (3160b02a + one
rmbp-queue doc commit).

## Finding (NETRING3, 2026-10-04)

The x86 ring-3 program window is 16 KiB in total. A TLS 1.3 client (`embedded-tls`: ~78 KiB code + ~20 KiB
buffers) cannot exist on the metal; LUMEN.BIN and VEIN.BIN hit the same wall.

## M1 — how a program is placed today, and what pins 16 KiB

* **The slot.** `arch/x86_64/memory.rs` keeps a STATIC pool of `USER_SLOTS = 12` address spaces. Each slot
  owns one PML4 (kernel half shared), one PDPT, one PD and ONE PT, plus a `.bss` backing of
  `USER_STATIC_SIZE = 0x149000` bytes. PML4[2] (USER_BASE = 1 TiB) → PDPT[0] → PD[0] → PT covers 2 MiB.
* **The window.** `build_slot` eagerly maps `U3_WINDOW_PAGES = 4` pages at USER_BASE: page 0 RX-RO, pages 1..3
  RW-NX; the initial RSP is the window top (`USER_BASE + 0x4000`). `syscall::USER_WINDOW_PAGES` mirrors it.
* **The FB hole is ABI and sits directly above the window.** The RO info page is at `base + 0x4000`, window
  surface slot 0 at `base + 0x5000` (WINX-1). Every shipped program derives `base` as `_start`'s address and
  hangs those landmarks off it (the `ASSERT(_start == 0)` in every `user-*-x86.ld`). **This is what pins 16
  KiB:** the program window cannot grow upward in place without moving `+0x4000`/`+0x5000`, which breaks every
  shipped binary. The slot table, the frame reservation (`.bss` per slot) and the single PT are secondary pins
  (all three would have to move for an in-place grow).
* **The loader** (`arch/x86_64/elf.rs`): static ET_EXEC, up to 8 PT_LOAD, bias = `base - min_vaddr` (linker
  scripts link at 0), every span must fit `user_window_size()` = 16 KiB, per-segment W^X via
  `protect_user_slot_range`, BSS zeroed (memsz > filesz). `load_program_common`, `read_el0_image` (shell
  `run`/`bg`) and the desktop-app reader also cap the FILE at `user_window_size()`.
* **Syscall buffers.** `user_range_ok` admits only `[USER_BASE, USER_BASE + 16 KiB)` (then the live-leaf walk
  decides). The FB hole is deliberately NOT a legal buffer.
* **Exit.** Every teardown funnels through `memory::free_user_space_by_cr3` (exit, KillSwitch reap, last
  `user_space_release` of an ELF-2 thread group), with the kernel CR3 already restored.
* **linuxabi** (`arch/x86_64/linuxabi/{mod,elf}.rs`) is the second model: a heap-allocated private PML4
  (`AddrSpace`), PT_LOAD mapped at its absolute low vaddrs (0x10000..16 MiB, splitting the identity map's huge
  pages), frames from `memory::alloc_page_frame` (the kernel heap, 4 KiB-aligned), tracked in a `BTreeSet`,
  freed at exit; a 128-page stack at a fixed top; brk/mmap regions. It is behind `UNAOS_LINUXABI`.

## Decision: (B) — size per program from the ELF, in a second window ("XWIN") above the FB hole

* **Layout.** The slot keeps everything it had: the classic 16 KiB window at `base`, the FB hole at
  `base + 0x4000 .. base + 0x149000`. NEW: the **ELF window** `[base + 0x200000, base + 0x200000 + 4 MiB)` =
  PD[1] and PD[2] of the slot's existing PDPT/PD, two PTs per slot in `.bss` (12 × 2 × 4 KiB = 96 KiB).
  `USER_XWIN_OFF = 0x200000` and `USER_WINDOW_BYTES = 4 MiB` live in `una-abi` (one number for kernel, user
  crates and arroyo).
* **Which model a program gets** is decided by where its ELF asks to be: `min PT_LOAD p_vaddr >=
  USER_XWIN_VA_X86` (= 0x10000200000, USER_BASE + 2 MiB) ⇒ **elf model**: the program LINKS at its real
  ring-3 address, so absolute pointers in its data (vtables, `core::fmt`, `&str` in statics) are right with
  no relocation — what a 78 KiB TLS client needs, and the linuxabi shape (fixed vaddrs). Anything else ⇒
  **fixed model**, the old path byte-for-byte.
  Every existing program (STAT, VUG/VUGC/VUGX/VUGK, PULSE, PREFS, HELLO.BIN flat) links at 0 and stays fixed.
* **Elf model.** Each PT_LOAD page is mapped on demand from the kernel heap (`alloc_zeroed`, 4 KiB-aligned —
  the same allocator linuxabi's `AddrSpace` uses), copied through the identity alias, BSS zeroed, leaf W/X from
  the segment flags (a page two segments share takes the union; W+X is refused). Stack at the XWIN top, sized
  by `PT_GNU_STACK.p_memsz` (`-z stack-size=`) or a 64 KiB default, clamped to 1 MiB, mapped eagerly, with one
  unmapped guard page beneath. Heap = `SYS_SBRK` from the page after the highest segment up to the guard.
  Its classic landmarks are at the fixed `una_abi::USER_BASE_X86 + 0x4000/0x5000` (it may still open windows).
* **Fixed model gets a heap too.** Its `SYS_SBRK` break starts at `base + 0x200000` and may use the whole 4 MiB
  XWIN (its stack stays in the classic window). So `alloc` works for a small program without relinking.
* **Cap.** Image span + stack (+ guard) > 4 MiB ⇒ refused at load with `-ENOMEM` and a line
  `:: RING3WIN: refused … -ENOMEM ::`; a heap past the guard ⇒ `SYS_SBRK` returns `-ENOMEM` with a line. An
  out-of-memory kernel heap mid-map unwinds the frames already taken and refuses the same way.
* **SYS_SBRK = 58** (after NETRING3's 56/57). `sbrk(delta)` → the OLD break VA, or a negative errno. delta 0
  queries; a negative delta frees whole pages above the new break.
* **Exit.** `free_user_space_by_cr3` calls `memory::xwin_free(s)` beside `clear_slot_fb(s)`: every XWIN leaf's
  frame is returned to the heap, both PD entries cleared, `AS_GEN` bumped (the SMPBAL-X86 argument covers
  other cores: a stale user translation is only consumable by dispatching a task, and dispatch reloads on a
  generation change).
* **Syscall buffers** in the XWIN are legal: `user_range_ok` admits `[base, base+16 KiB) ∪ XWIN`, and the
  live-leaf walk still decides (unmapped guard / heap tail ⇒ `-EFAULT`). The FB hole stays excluded.
* **File cap.** The shell/`load_program_common`/desktop-app readers move from `user_window_size()` (16 KiB) to
  `user_image_cap()` = `USER_WINDOW_BYTES`.

### Why not one mapper with linuxabi (R28), and the cost either way

The ask was "one mapper, two personas". It is NOT shared code this arc, and the reason is structural: linuxabi's
`AddrSpace` owns the whole PML4 and maps in the LOWER half (it splits the identity map's huge pages per process),
while a UnaOS program's identity is its SLOT — `current_slot()` matches the live CR3 against the static slot
pool, and the handle table, the window table, the bus row, the focus id and the input ring are all keyed by that
index. Moving UnaOS programs onto `AddrSpace` means re-keying all of those (an arc of its own), and linuxabi is
knob-gated (`UNAOS_LINUXABI`) so the default UnaOS loader cannot link it today. What IS shared: the allocator
(kernel heap 4 KiB frames, freed at exit), the model (PT_LOAD at its vaddrs, frames on demand, stack + brk), the
W^X rules. **Cost of (B) as built:** +96 KiB `.bss` (PTs), ~0 for existing programs (no frames taken unless a
program asks), up to 1024 heap frames (4 MiB) per elf-model program; 12 slots × 4 MiB = 48 MiB worst case
against a multi-GiB heap. **Cost of (A)** (grow the fixed window to 256 KiB): moves the FB ABI offsets — every
shipped binary relinks — and 12 × 256 KiB = 3 MiB of eager frames whether used or not; rejected.
**Owed:** fold `elf.rs`'s validator and linuxabi's `parse` into one `crate::elf` (the doc at the top of
`elf.rs` already flags the aarch64 twin the same way).

### aarch64

The aarch64 loader is separate (`arch/aarch64/syscall.rs` `validate_elf`/`map_image_into_slot`, `uslots`).
It is UNCHANGED: 16 KiB, its arroyo asserts (`USER_REGION_SIZE (16384)`) untouched. `SYS_SBRK` on aarch64 falls
to the default arm (`-ENOSYS`) — the number is minted in `una-abi` for both arches; the aarch64 body is owed.

### No knob

A bigger window is the product; existing programs take the unchanged fixed path, so there is nothing to arm.
The only new reachable surface for an old program is `SYS_SBRK` (which it never calls) and a wider
`user_range_ok` (which only admits pages the program itself mapped).

## Milestones

* M1 — this design.
* M2 — kernel: `memory.rs` XWIN (map/free/sbrk), `elf.rs` elf-model validate + map, `syscall.rs` dispatch arm,
  `user_range_ok`, `user_image_cap()`, teardown hook.
* M3 — arroyo: `USER_WINDOW_BYTES` beside the kernel constant; the x86 user-ELF asserts become one function
  (`user_elf_window_check`) that reads the PT_LOAD layout and enforces `fixed: span <= 16384` / `elf: span +
  stack <= USER_WINDOW_BYTES`; `scripts/window-parity.sh` proves una-abi, arroyo and the kernel agree.
* M4 — `crates/user-big` → `APPS/BIG.BIN`; `tests ring3win`.

## Witness

`:: RING3WIN: model=elf window=4194304 big_ok=1 sbrk=<bytes> freed=1 -> PASS ::`
(fixture: `tests ring3win` — reads `/apps/BIG.BIN`, runs it, checks the exit checksum against the kernel's own
computation, waits for the slot release and asserts the XWIN live-frame count returned to its value before the
launch.)

## M2–M4 as built

* Kernel (`arch/x86_64/memory.rs` tail): `SLOT_XPT[12][2]` static PTs, `xwin_map_page` / `xwin_copy_in` /
  `xwin_free` / `xwin_set_heap` / `sys_sbrk`, live-frame counter `xwin_live_pages()`; teardown hook on the
  `clear_slot_fb` line of `free_user_space_by_cr3`. `elf.rs`: `PT_GNU_STACK` read, `validate_elf_model`
  (rebases to window offsets), `map_elf_model`; the fixed path also arms the XWIN heap. `syscall.rs`:
  `SYS_SBRK` arm (same-line fold on `SYS_CLOSE`), `user_range_ok` admits the XWIN, `user_image_cap()`;
  `shell.rs` / `desktop_uefi.rs` readers cap at it. Load line `:: RING3WIN: model=elf slot=… segs=… span=…
  stack=… heap=[…) frames=… ::`; refusals `:: RING3WIN: refused … -ENOMEM ::`.
* arroyo: `USER_WINDOW_BYTES=4194304`, `USER_XWIN_VA_X86`, `user_elf_window_check` (replaces the four x86
  `[ size -le 16384 ]` asserts; reads the PT_LOAD layout: fixed span <= 16384, elf span + stack + guard <=
  cap), `build_user_big_x86` called beside `build_user_prefs_x86` at all five sites; `check` runs
  `scripts/window-parity.sh` beside knob-parity (una-abi = kernel-derived = arroyo = builder; negative
  control: arroyo at 16384 ⇒ rc=1).
* `crates/user-big` → `target/BIG-X86.ELF` → `APPS/BIG.BIN` (ESP + DATA). lld drops PT_GNU_STACK's size when
  the script has a PHDRS clause (measured: p_memsz 0), so `user-big-x86.ld` has none and lld fills it from
  `-z stack-size=0x40000`.

### Every x86 program against the new cap (measured by `user_elf_window_check`)

| image | file | model | in-window span | against |
|---|---|---|---|---|
| STAT-X86.ELF | 8472 | fixed | 4112 | 16384 |
| VUG-X86.ELF | 12664 | fixed | 13588 | 16384 |
| VUGC-X86.ELF | 12664 | fixed | 12540 | 16384 |
| VUGX-X86.ELF | 12664 | fixed | 13588 | 16384 |
| VUGK-X86.ELF | 12664 | fixed | 12832 | 16384 |
| PULSE-X86.ELF | 12568 | fixed | 9760 | 16384 |
| PREFS-X86.ELF | 8704 | fixed | 9428 | 16384 |
| HELLO.BIN (flat) | 72 | flat | 72 | 4096 |
| BIG-X86.ELF | 12640 | elf | 73744 + 262144 stack + 4096 guard = 339984 | 4194304 |

All existing programs are unchanged (fixed model, same bytes, same layout). NET.BIN / LUMEN.BIN / VEIN.BIN are
in parallel arcs and not in this tree; MIDDEN.BIN is aarch64 (untouched).

### NETRING3's TLS note

`tls=skip reason=window` → **unblocked: the spike fits.** `embedded-tls` ≈ 78 KiB code + ≈ 20 KiB buffers ≈
98 KiB of image span; linked at `USER_XWIN_VA_X86` with the 64 KiB default stack and a guard page that is
≈ 167 KiB against a 4 MiB window (≈ 4 %), and its record buffers can live on the `SYS_SBRK` heap instead. NET.BIN
moves to the elf model by swapping its linker script for the `user-big-x86.ld` shape (absolute link VA, no
PHDRS) — no kernel knob. (NETRING3's branch is not edited here; the fold carries this.)

## Proof (metal; R78 — no QEMU)

`tests ring3win` on a boot whose DATA volume carries `APPS/BIG.BIN`:

```
:: RING3WIN: model=elf slot=<s> segs=3 span=73744 stack=262144 heap=[0x10000213000,0x100005bf000) frames=<n> ::
BIG: static=ok fnv=<n> stack=ok sbrk=<>=102400> cap=refused
:: RING3WIN: sbrk refused slot=<s> brk=… delta=8388608 cap=… -ENOMEM ::
[ring3win] status=Some(..) bits=0xf fnv_want=… ck_ok=true live0=0 live1=0 freed_pages=<n> heap=<bytes>
:: RING3WIN: model=elf window=4194304 big_ok=1 sbrk=<bytes> freed=1 -> PASS ::
```

Without BIG.BIN on the volume: `:: RING3WIN: … reason=no-big-bin -> SKIP ::`.

## Owed

* aarch64: the ELF window and `SYS_SBRK` (the number is minted; the arm answers `-ENOSYS`).
* One `crate::elf` validator for x86 UnaOS, aarch64 UnaOS and linuxabi (the R28 "one mapper" fold).
* `SYS_SBRK` on a fixed-model program is reachable but untested by a shipped program (BIG is elf-model).
* Free-on-shrink is page-granular and the user allocator in BIG is a bump (no free); a real user `alloc`
  crate (linked_list over sbrk) is the next consumer's to choose.
