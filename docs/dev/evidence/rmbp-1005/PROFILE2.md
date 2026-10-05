# PROFILE2 — stacks, names, ring-3 symbols, the Linux hook, SYS_PROF (rmbp-ledger B340)

Branch `exec-rmbp-profile2`, cut from `5e91c65b` (the branch tip after boot 21's fold). Answers what PROFILE
(B331, `PROFILE.md` beside this file) left owed.

## Finding

PROFILE samples are depth 1: they say which function the timer interrupted, never who called it, so a hot
`memcpy` or spin-lock reads as itself and not as the compositor pass that drove it. `prof tasks` prints task
ids (there was no safe cross-core tid -> name lookup). Ring-3 samples are counted as `[ring3]` and never
symbolised. A Linux-ABI task's syscalls return through `linuxabi::dispatch` BEFORE the latency hook, so they
are invisible to `prof sys`. And a program cannot profile itself.

## The seam

`CHARTER: Kernel — kernel-by-ruling` (R83), unchanged from PROFILE: the sampler lives in Ring 0, the symbol
tables live beside the build artifacts, and the host script (`tools/flame`) does every join. The kernel still
embeds no symbol table. `SYS_PROF` is the ring-3 face of the same rings (`una_abi::prof`); no second store.
No new file under video/, fs/, install/, selfhost/ or the shell files. No new knob, no new shell verb.

## Milestones

* **M1 STACKS.** A frame-pointer walk in the sampler, bounded depth 32. x86: the walk starts at the
  sampler's own `rbp` and climbs to the frame whose return slot equals the interrupt frame's RIP (the
  `x86-interrupt` handler's frame record sits directly on the hardware frame), then follows the
  interrupted context's chain. aarch64: the same, with the boundary at the frame whose return address lies
  in `__vec_irq` (the IRQ stub leaves `x29` untouched, so the handler's frame record holds the interrupted
  `x29`). EVERY frame pointer is checked before it is dereferenced: 8-aligned, at or above the live stack
  pointer, and 16 bytes below the top of the current stack (the current task's stack from its `Task` —
  a same-core read with interrupts masked; the BSP's boot stack `[sp, top recorded at _start)`; an AP's
  static boot stack), strictly increasing; every return address must lie in the kernel image's executable
  `PT_LOAD` range (from `__ehdr_start`, computed at `prof start`) or the walk stops. Ring-3 contexts are
  not walked. No fault is possible in the ISR: nothing is read that the bounds did not prove mapped. The
  rings become variable-length (header word, RIP, callers), 32768 words per CPU, heap-allocated at the
  first `prof start` as before. The metal shape builds with `-C force-frame-pointers=yes` (arroyo's
  `KERNEL_FP_RUSTFLAGS` on the QEMU kernel line; the builder sets the same RUSTFLAGS on its kernel
  `cargo`, which is the build the ESP carries). `tools/flame` folds the stacks and draws a real
  (nested) flame graph; `--folded` writes the folded-stacks text.
* **M2 TASK NAMES.** A lock-free name table, 256 fixed slots keyed `tid % 256`, 16-byte names in two
  atomics, a seqlock on the slot's tid word. Written at spawn (`note_task` folded onto the
  `NEXT_TID.fetch_add` line of each spawn path, both arches); a program launch (`run`, `bg`, a bare name)
  arms a per-CPU pending program name the next ring-3 spawn on that CPU consumes, so a program's task
  carries its program name (`LUMEN`) and a program id (a 63-slot interned table); its threads inherit
  both. `prof tasks` prints `name=` beside every tid.
* **M3 RING-3 SYMBOLS.** arroyo's `emit_user_syms <unstripped ELF> <NAME>` writes
  `target/APPS/<NAME>.syms` (`llvm-nm -n -C --defined-only`, BEFORE the `llvm-objcopy --strip-all`) in
  every x86 `build_user_*_x86`. Each sample records the task's program id; `prof dump` prints the program
  table (`[prof] prog id= name=`) and `tools/flame` resolves a ring-3 RIP against `APPS/<name>.syms`
  (an elf-model program links at its absolute VA; a classic program links at 0 and runs at the window
  base, which flame subtracts when the RIP is past the table).
* **M4 LINUX HOOK.** `syscall_dispatch`'s Linux branch is timed by the same `sys_t0`/`sys_note` pair,
  into its own table (Linux numbers, 0..335, the last pooled). `prof sys` prints those rows as
  `abi=linux`.
* **M5 SYS_PROF.** `SYS_PROF(op, buf, len)`, `una_abi::prof`: `OP_START` arms the sampler if idle (the
  caller owns that run), `OP_STOP` disarms a run the caller owns, `OP_READ` copies at most
  `prof::READ_MAX` of the caller's own samples (its tid, or its program id) as 24-byte
  `una_abi::prof::Sample` records into the caller's buffer (a bounded copy, validated by the arch's own
  `copy_to_user`), `OP_STATUS` returns the caller's sample count. `tests prof2` is the witness.

## Witness

`:: PROFILE2: stacks=<n> depth_max=<n> names=<n> r3_syms=<ok|skip> linux_hook=1 sys_prof=1 -> PASS ::`
(`tests prof2`, never at boot). It spawns a kernel task `prof2-load` that runs the synthetic load three
frames deep for 600 ms on another core with the sampler armed; `stacks` counts samples with at least one
caller frame, `names` the sampled tids the name table resolves, `r3_syms=ok` when a ring-3 sample in the
window carried a program id (`skip` when no ring-3 code ran), `linux_hook=1` when the Linux table took a
note (`skip` where linuxabi is not built), `sys_prof=1` when `SYS_PROF`'s read core returned only the load
task's samples and at least one.

## Owed

Ring-3 STACKS (user frames are not walked: the ISR reads no user memory). The `[wc-h]` present-time cost of
the frame pointers needs the metal boot (see Status). aarch64 builds without frame pointers (the walk
compiles, and yields depth 0 until its kernel line carries the flag). Ring-3 `.syms` for the aarch64
programs.
