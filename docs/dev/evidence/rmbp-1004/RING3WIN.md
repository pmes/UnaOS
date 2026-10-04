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
* **Which model a program gets** is decided by where its ELF asks to be: `min PT_LOAD p_vaddr >= 0x200000` ⇒
  **elf model** (p_vaddr is the offset from the window base; the classic `base - min_vaddr` bias equals that
  for every shipped image, which links at 0). Anything else ⇒ **fixed model**, the old path byte-for-byte.
  Every existing program (STAT, VUG/VUGC/VUGX/VUGK, PULSE, PREFS, HELLO.BIN flat) links at 0 and stays fixed.
* **Elf model.** Each PT_LOAD page is mapped on demand from the kernel heap (`alloc_zeroed`, 4 KiB-aligned —
  the same allocator linuxabi's `AddrSpace` uses), copied through the identity alias, BSS zeroed, leaf W/X from
  the segment flags (a page two segments share takes the union; W+X is refused). Stack at the XWIN top, sized
  by `PT_GNU_STACK.p_memsz` (`-z stack-size=`) or a 64 KiB default, clamped to 1 MiB, mapped eagerly, with one
  unmapped guard page beneath. Heap = `SYS_SBRK` from the page after the highest segment up to the guard.
  The program finds its classic landmarks as `base = _start_page - 0x200000` (it may still open windows).
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
