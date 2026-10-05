# WINDOW2 — the ring-3 window raised to 64 MiB (rmbp-ledger B361, R85)

Branch `exec-rmbp-window2`, cut from the boot-23 fold c8c1658d. CHARTER: Kernel — kernel-by-ruling (R85 item 9:
"i'm guessing the window will need to be raised might as well do it now"). No knob: a bigger window is the
product, and every existing program takes the same path it took before.

## Design

**Finding.** Three consumers hit two walls. (1) `una_abi::USER_WINDOW_BYTES` = 4 MiB (RING3WIN, B316): HOLOCRON2
(B355) had to run Argon2id in the kernel through `SYS_KDF` (66) because its 19..64 MiB does not fit; KERNELFONT
(B359) could not give the Lumen window a 343 KB face beside TLS (LUMEN's span is already 3.35 MB of the 4 MiB).
(2) `linuxabi/elf.rs` `IMAGE_LIMIT` = 16 MiB for fixed-address Linux images: SELFBUILD6's probe (B360) found the
68.5 MB LLD.LNX, linked at 0x400000, refused at load (`segment outside the loadable window`), so SELFBUILD5's
`lld=` could not pass as shipped.

**What pins each number, and the seam.**
* *The UnaOS window* is VA-only: x86 maps it at `USER_BASE + 2 MiB` (PD entries 1..=N of the slot's PD), aarch64
  at the extension GiB + 2 MiB (L2 entries 1..=N). Both arches size it from the one una-abi constant;
  `scripts/window-parity.sh` holds una-abi, kernel, arroyo and builder to it. What did NOT scale: the page
  tables behind it were static `.bss` per slot (one PT per 2 MiB) — at 64 MiB that is 12 x 32 x 4 KiB = 1.5 MiB
  on x86 and 8 x 32 x 4 KiB = 1 MiB on the Pi, whose `.bss` has a hard ceiling at its hand-placed 32 MiB heap.
  **Seam: Kernel — driver**: the tables become heap frames wired on the first touch of their 2 MiB and freed by
  the teardown that already frees the data frames (`xwin_free` / `slot_free`). Page-table span checked: x86
  1 + 32 PD entries of 512 (one PD covers 1 GiB); aarch64 1 + 32 L2 entries of 512 (one L2 covers the
  extension GiB). The aarch64 launcher's file cap is bounded by a quarter of its 48 MiB heap (12 MiB): the
  launcher reads the whole file into the heap and the window's frames come from it too.
* *The Linux fixed-image window* is NOT VA-only: the process's low PML4[0] view REPLACES the kernel's identity
  map under its CR3, so every page there must be RAM the kernel never touches through the identity map while
  the process runs. Raising `IMAGE_LIMIT` to 64 MiB therefore needs two more facts: the image range is outside
  the kernel heap (the rMBP heap is at 0x20200000, 514 MiB, but a small-RAM machine's could be below 64 MiB —
  `low_window_ok` refuses instead of overlaying it), and the user frame pool (`vm.rs`, identity-reached
  frames) starts at `IMAGE_LIMIT` instead of 16 MiB. Even at 64 MiB the 68.5 MB lld does not fit — hence R85's
  "relinked as PIE": **a static-PIE (ET_DYN, no PT_INTERP) is placed at `PIE_BASE` = the top 4 GiB of the
  process's private PML4[2] half** (above `vm::MMAP2_LIMIT`, never inherited from the kernel, so no RAM check
  applies), span cap 1 GiB, and relocates itself (glibc's `_dl_relocate_static_pie`); the kernel only biases
  the plan's vaddrs, entry and AT_PHDR.
* *Holocron* (**Holocron — shared-core**, `holocron_core` + CRYPTOCORE unchanged): `MetalSealer::derive_key`
  runs `crypto_core::argon2::argon2` in ring 3 over memory lent by `SYS_SBRK` (the size-class heap stops at
  8 MiB classes) and handed straight back. New rings are made at `METAL_KDF` = 48 MiB, t 3, p 4 (RFC 9106's
  second option with the memory the window holds beside image, heap and stack; above the 19 MiB floor). A ring
  whose recorded memory the window cannot hold (64 MiB: made by the host daemon, or by boot 23's HOLOCRON.ELF
  through SYS_KDF) unlocks through SYS_KDF and says so — the only remaining Holocron use of 66.

**Milestones.**
* M1 — `USER_WINDOW_BYTES` = 64 MiB (una-abi, arroyo, builder; parity script green); x86 + aarch64 tables on
  demand; refusal strings; `userspace.md` §"The ring-3 ELF window"; VUG/LUMEN/NET/DIAG/HOLOCRON/BIG rebuilt
  through arroyo's own functions (sizes below).
* M2 — `linuxabi`: `IMAGE_LIMIT` = 64 MiB with `low_window_ok` and the pool floor; static-PIE at `PIE_BASE`;
  arroyo `build_selfbuild5_x86` builds LLD.LNX static-PIE (`-fPIE`, `-static-pie`, `LLVM_ENABLE_PIC=ON`,
  `CMAKE_SKIP_RPATH=ON`) and its acceptance test wants ET_DYN with no INTERP and no NEEDED.
* M3 — HOLOCRON.ELF derives in ring 3; wire `:: HOLOCRON: serve … kdf=ring3 sys_kdf=unused ::`.
* M4 — `tests window` (`src/window2.rs`) + BIG.ELF's 48 MiB leg.

**Witness (metal; R78 — no QEMU).** `tests window`:
```
BIG: static=ok fnv=<n> stack=ok sbrk=<n> cap=refused alloc48m=ok pages=12287
[window2] big status=<s> bits=0x1f
[holocron] kdf-selftest where=ring3 m_kib=49152 t=3 p=4 ms=<n> -> derived
[window2] kdf ring3_status=<s> want=<w> ring3_ms=<n> kernel_ms=<n>
[window2] fixed20m span=20975616 exit=42 ms=<n>
[window2] lld size=<n> entry=<va> segs=<n> base=0x17f00000000
[window2] xwin_pt_live before=0 after=0
:: WINDOW2: bytes=67108864 image_limit=67108864 alloc48m=ok fixed20m=ok lld_pie=ok kdf_ring3=ok -> PASS ::
```
A leg whose fixture is not staged prints `skip(<why>)` and the verdict is SKIP (BIG.ELF, HOLOCRON.ELF,
LLD.LNX; `fixed20m` writes its 4 KiB image to `<home>window2-fixed20m.lnx` and deletes it). `tests ring3win`
keeps its own line (it reads only BIG's low four bits).

**Stays owed.** The metal boot. `IMAGE_LIMIT` at 64 MiB only helps an ET_EXEC whose range the firmware map calls
Usable (the rMBP's low Usable map above 1 MiB is not in the flight logs — `fixed20m` will say). One
`crate::elf` validator for x86, aarch64 and linuxabi (RING3WIN's owed fold, still owed). KERNELFONT's face in
Lumen is KERNELFONT2's (B363) to take now that the window holds it. The dynamic loader (SELFBUILD6 M2) is not
this arc.
