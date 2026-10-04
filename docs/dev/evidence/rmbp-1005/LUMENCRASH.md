# LUMENCRASH — `lumen` died at entry+0x3f with a #GP (rmbp-ledger B326)

CHARTER: Kernel — driver (the ring-3 program ABI: the x86 user target and the static checks on the images it
builds). Branch `exec-rmbp-lumencrash`, cut from 4e48ab03 (merge11).

## Finding (FLIGHT20, boot 20)

`lumen` → `:: BGRUN: bg /apps/LUMEN.BIN — loaded 12672 bytes, entry 0x10000000000, pid=45` →
`:: RING-3 FAULT: task 'bg-user' KILLED — vec=13 err=0x0 rip=0x1000000003f cr2=0x0 ::`, twice.

## M1 — the faulting instruction, and why it #GPs

The flown image was rebuilt from 776ffbe7 with its own arroyo line (`build_user_lumen_x86` there); the stripped
product is **12672 bytes, byte-for-byte the size boot 20 loaded**. `llvm-objdump -d` around entry:

```
      31: 49 89 c6                     	movq	%rax, %r14
      34: 4c 8b 2d c5 ff ff ff         	movq	-0x3b(%rip), %r13       # 0x0 <.text>
      3b: 6a 01                        	pushq	$0x1
      3d: 41 5f                        	popq	%r15
      3f: 45 89 7d 04                  	movl	%r15d, 0x4(%r13)        <- rip=entry+0x3f, #GP(0)
```

Source: `wit(W_WINDOW, 1)` — a store to `WIT[1]`, the `#[no_mangle] static mut WIT` in `.data.lumenwit`.
Relinked with `--emit-relocs`, the load at +0x34 carries **`R_X86_64_GOTPCREL WIT`** (five sites). Three facts
make that a crash:

1. `x86_64-unknown-none.json` (every x86 user crate's target) leaves `relocation-model` at rustc's default,
   `pic`, so an EXPORTED static (`#[no_mangle]`, not dso_local under PIC) is reached through a GOT slot.
2. A custom target's `relax-elf-relocations` defaults to **false**, so LLVM emits the plain `GOTPCREL`, which
   lld may not rewrite into `leaq WIT(%rip)` (it relaxes only `GOTPCRELX`/`REX_GOTPCRELX`).
3. Every `user-*-x86.ld` (and the aarch64 twins) lists `*(.got)` under `/DISCARD/`. lld drops the GOT and
   resolves the slot's address to **0**, silently: the load reads the program's own first 8 code bytes
   (`55 41 57 41 56 41 55 41`, `push %rbp; push %r15; …`), so `%r13 = 0x4155415641574155`, a NON-CANONICAL
   address. A store through a non-canonical address is #GP(0), not #PF — hence `vec=13 err=0x0 cr2=0x0`.

Not the stack, not a syscall shape, not a segment op: the kernel's RSP (fixed model: window top − 16; elf model:
`map_elf_model`'s `(base + XWIN top) & !0xF`, eagerly mapped, PT_GNU_STACK honoured — RING3WIN proved it with
BIG.ELF) is inside a mapped page in both models, and the soft-float target means a 16-vs-8 misalignment cannot
fault.

Why only the FIXED model fails silently: there the program links at 0, so the discarded GOT's address 0 is in
range of every RIP-relative load and lands on `_start`. In the ELF-window model (linked at 0x10000200000) the same
reference cannot reach VA 0 and lld refuses the link (`relocation R_X86_64_GOTPCREL out of range … references
'BRK_CUR'` — measured by adding one `#[no_mangle]` static to a scratch copy of user-big).

**In this tree** the instruction is gone: LUMENAPP's LUMEN.ELF has no exported static (its +0x3f is a `callq`
to `Line::wire`), and no other x86 user crate has one. The cause is latent, not fixed: the next `#[no_mangle]`
static, or an `extern` static from a library, in ANY x86 user program crashes the same way at its first touch.

## The seam

Kernel — the one x86 user target spec all ring-3 programs build against, plus the build's static gate. No
kernel spawn change: both models hand a correct RSP.

## Milestones

* **M1** DIAGNOSE (this section).
* **M2** FIX — `x86_64-unknown-none.json` gains `"relax-elf-relocations": true`: LLVM emits `REX_GOTPCRELX`,
  lld (static, non-PIE, every symbol non-preemptible) rewrites each GOT load into a direct `leaq sym(%rip)`,
  so no GOT exists to discard. Proven on the flown source: the same +0x34 becomes
  `leaq 0x1fc5(%rip), %r13  # 0x2000 <WIT>`; and the scratch user-big with a `#[no_mangle]` static, refused by
  lld before, links and passes M4. One file fixes every x86 user crate (lumen, prefs, net, big, vug,
  pulse, stat, hello).
* **M3** WITNESS — `tests lumen` gains a SPAWN step: it spawns the real LUMEN.ELF through
  `spawn_user_image_bg` (what `bg` and the bare-name launch call), waits ≤ 2 s for the program's first wire line
  `:: LUMEN: start …` on its console row or a ring-3 fault of its pid, then kills the job:
  `:: LUMENCRASH: spawned=1 first_line=<ok|fault vec=N rip=+0x..|timeout> -> PASS|FAIL ::` (rip relative to
  entry).
* **M4** STATIC GATE — `scripts/elf-entry-check.sh <elf>…` (called by arroyo's `user_elf_entry_check` from
  every x86 ELF build): refuses (a) any RIP-relative operand whose target lies outside every PT_LOAD (a
  discarded section's address), (b) any non-`lea` RIP-relative read of the first 64 bytes at the entry (the
  GOT-at-0 signature that flew), (c) in the first 64 bytes at the entry, an instruction class ring 3 cannot
  execute (privileged, port I/O, segment/descriptor loads, far transfers, `ud2`/`int3`/`hlt`).

## Witness

`:: LUMENCRASH: spawned=1 first_line=ok -> PASS ::` (after `:: LUMEN: start provider=… ::` from the program).

## Owed

aarch64 images link `relocation-model: static` (`aarch64-base.json`) and reach statics with `adrp`, so the GOT
class does not arise there today; the M4 gate is x86-only. The `/DISCARD/ *(.got)` lines stay (with M2 there is
no GOT to discard and M4 refuses an image if one ever comes back).

## Results (no QEMU, R78)

* M4 on the flown image (776ffbe7 source, stripped, 12672 B): `refused got-at-0 at entry+0x34 — movq -0x3b(%rip),
  %r13 # 0x0` plus the four other `WIT` sites → `-> FAIL`; the same source with M2's target → `-> PASS`.
* Every x86 image this tree stages, built by its arroyo function with M2 + M4: STAT, VUG, VUGC, VUGX, VUGK, PULSE,
  LUMEN (elf model, 127704 B), NET, PREFS, BIG — all `:: ELFENTRY: … -> PASS ::`.
* Compile legs: x86 metal shape (…,lumen,netring3,prefs_reset,census,installdemo,instgui,witness) exit 0;
  aarch64 `login,loginst,virt_el0,lumen` exit 0.
* The metal wire for `tests lumen`: `:: LUMENAPP: image=/apps/LUMEN.ELF window=elf … -> PASS ::`, the program's
  own `:: LUMEN: start provider=… ::`, `[lumencrash] pid=… slot=… entry=0x10000200000 wait_ms=2000 kill=…`,
  `:: LUMENCRASH: spawned=1 first_line=ok -> PASS ::`.
