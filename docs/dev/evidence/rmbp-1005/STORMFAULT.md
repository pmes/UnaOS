# STORMFAULT — rmbp-ledger B351

Branch `exec-rmbp-stormfault`, cut at 452a6127, merged with `exec-rmbp-merge12` (tip e7350187).

## Finding

Boot 21 (FLIGHT21 §2): `storm` printed `:: STORM: launched 6/6 vugs ::`, then six
`:: RING-3 FAULT: task 'bg-user' KILLED — vec=14 err=0x6 rip=0x10000001845 cr2=0x10000056000 ::`.
cr2 is the window base plus 0x56000.

B351 guessed that the `.ELF` loader drops the bss or stack. **The real headers say it does not.**
`readelf -lW` on this tree's images:

| image | LOAD (offset, vaddr, filesz, memsz, flags) | GNU_STACK | entry |
|---|---|---|---|
| VUG-X86.ELF (APPS/VUG.ELF) | 0x0, 0x0, 0x2af8, 0x2af8, R E · 0x3000, 0x3000, 0x1c, 0x514, RW · NOTE at 0x2ae0 | none | 0xe8 |
| LUMEN-X86.ELF (APPS/LUMEN.ELF) | 0x1000, 0x10000200000, 0x2eb05, 0x2eb05, R E · 0x2fb08, 0x1000022eb08, 0x32de, 0x32de, R · 0x33000, 0x10000232000, 0x790, 0x2ee0a0, RW | 0x40000 | 0x10000200000 |
| VUG.ELF (aarch64) | 0x1000, 0x0, 0x1f68, 0x1f68, R E · 0x3000, 0x2000, 0x0, 0x4ac, RW | none | 0x0 |

VUG's data segment ends at window offset 0x3514. The fixed-model loader zeroes the whole 16 KiB window
and every segment's memsz, so the bss is mapped and zero. The address 0x56000 is far outside the image.
It is the first byte after **surface slot 0**: 0x5000 + 288·288·4 = 0x5000 + 0x51000 = 0x56000.

The root cause is in VUG, not the loader. On x86, `user-vug/src/main.rs` worked out the window base as
`_start - 0xb0`, where 0xb0 was SIZEOF_HEADERS (64 + 2·56). EXECNAME (B322) added a third program header,
the PT_NOTE that carries `.note.unaos.app`. That made SIZEOF_HEADERS 0xe8, and `_start` moved to 0xe8.
The link-script guard `ASSERT(_start == SIZEOF_HEADERS)` moved along with it, so it still passed while the
literal went stale by 0x38. The surface base became window+0x5038, and the last rows of the 288×288 blit
in `render_band` (stride 0x480) crossed the end of the mapping at exactly window+0x56000. This is
WINX-8's second corpse again, at a new width. Boot 20 ran images built before the note existed.

## Seam

`unaos/libs/sys/elf_core` (CHARTER: Kernel — shared-core). It is a no_std crate with no dependencies,
in the midden_core/diag_core shape. It holds the ring-3 ELF **segment plan**, `elf_core::plan`:
- the full memsz span of every PT_LOAD, with its zero-filled tail;
- the PT_GNU_STACK stack (or the default), plus the guard page in the ELF window.

The plan refuses each of these with a named `PlanErr`:
- a segment that starts below the window, or whose memsz leaves it;
- overlapping segments (ELF window only — WINX-3's edge rung keeps loading in the classic window);
- a segment, stack or guard page over the args page;
- a stack request over the cap;
- a stack that collides with the bss.

It also provides `classify`, the `seg=bss|stack|none` classifier. The x86 loader (`arch/x86_64/elf.rs`)
and the aarch64 ELF window (`arch/aarch64/xwin.rs`) both call this one plan. The host test runs it over
the headers in the table above.

## Milestones

- **M1**: user-vug x86 takes the window base from `__ehdr_start` with an explicit RIP-relative `lea`
  (`lea -0x100(%rip)` at 0xf9 → `__ehdr_start` = 0). The link script asserts `__ehdr_start == 0`. All four
  x86 images (VUG/VUGC/VUGX/VUGK) build. The aarch64 VUG has entry 0, so `base = _start` is still correct
  there.
- **M2**: `elf_core` plus `cargo test -p elf_core` (6 tests). `STORMFAULT_VUG=` and `STORMFAULT_LUMEN=`
  plan the real built bytes.
- **M3**: the x86 `validate_elf` and `validate_elf_model` run the plan. Every load prints
  `:: STORMFAULT: plan model=… segs=… bss=… stack=[…) guard=… ::` (capped at 32 lines). A refusal prints
  `:: STORMFAULT: refused model=… reason=<word> ::` and the loader returns the named error. Each slot keeps
  a fault map, which slot teardown clears. The ring-3 fault line gains `seg=`. `tests elfbss` is added.
- **M4**: aarch64 `xwin.rs` `place()` runs the same plan.

## Witness

Host: `:: STORMFAULT-HOST: vug max_end=0x3514 bss=[0x301c,0x3514) stack=[0x3520,0x4000) cr2_off=0x56000 seg=none -> PASS ::`

Metal (boot 22):
- `storm` → `:: STORM: launched 6/6 vugs ::` with **no** `RING-3 FAULT` line, and six live vug windows
  until `kill`.
- `:: STORMFAULT: plan model=fixed segs=2 bss=1272 stack=[0x3520,0x4000) guard=false ::` per vug launch.
- `tests elfbss` → `:: STORMFAULT: elfbss bss=65536 exit=0 seg_last=bss refuse_window=1 refuse_stack=1 -> PASS ::`.
- Any ring-3 fault line from now on ends in ` seg=bss|stack|none ::`.

## Owed

- The aarch64 `tests elfbss` fixture.
- `seg=` on the aarch64 `:: EL0 FAULT:` line, which needs a per-slot map in xwin/uslots.
- One ELF validator for x86, aarch64 and linuxabi. It is flagged since WINX-2, and this arc shares only
  the plan.
