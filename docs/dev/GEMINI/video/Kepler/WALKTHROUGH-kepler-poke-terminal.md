# WALKTHROUGH — ECHO/POKE split, and the terminal falcon read of 0x409504

Branch `wt/kepler-poke-x86`. One file changed:
`unaos/crates/kernel/src/drivers/gpu/kepler.rs`.

| Commit | Title |
|---|---|
| `0a84a29e` | `docs/kepler: plan the ECHO/POKE split before writing any of it` |
| `c071cd09` | `gpu/kepler: split the 0x409504 read out of ECHO into a terminal POKE image` |

## 1. What was wrong

`ECHO_A_BYTES` — the image that exists to answer "does the FECS falcon execute
at all?" — carried this inside its `cmd == 1` arm:

```
0x10 | f1 38 00 41 | mov   $r8, 0x4100
0x14 | f0 38 01    | sethi $r8, 0x01
...
0x4a | cf 84 00    | iord  $r4, I[$r8]     ; 0x409504
```

Two things are wrong with that at once.

**It poisons the unit it is testing.** The image runs from the `ucode-echo`
leg, mid-sequence, ~400 lines of boot before the terminal poke. The first
access to `0x409504` (WRCMD_CMD) faults and wedges every subsequent read in the
FECS unit for the rest of the boot — s31 discovered it, s32 confirmed it with
its own control frame, s34 convicted it by elimination
(`docs/dev/OS/08_VIDEO/falcon_microcode_spec.md` §5.4). Every FECS observation
printed after the echo leg — the `ucode-post` sweep, `recon-post`, the ledger —
was therefore taken through a wedged unit.

**Two of those four instructions are not the instructions the comments name.**
`f1 38 00 41` is not `mov $r8, 0x4100` (that is `f1 87 00 41`) and `f0 38 01` is
not `sethi $r8, 0x01` (that is `f0 83 01`). The same file also had `f4 2b 2e`
where `bra eq` is `f4 0b …`, and `b0 52 01` where `sub b32` is `b6 52 …`. Four
malformed instructions, three review rounds, because the assertions checked
immediates — `port_i16(&ECHO_A_BYTES, 0x10) == 0x4100` passes on `f1 38 00 41`
just as happily as on `f1 87 00 41`, since it only looks at bytes 2 and 3 — and
the comments read correctly.

## 2. What landed

**`ECHO_A_BYTES`** — no `$r8`, no `iord I[$r8]`, no `0x409504`. It acks with
the literal `$r3 = 1` (`iowr I[$r2], $r3`). The absence is asserted, not
promised:

```rust
assert!(!contains4(b, &mov_i16(8, (IO_WRCMD_CMD & 0xFFFF) as u16)));
assert!(!contains3(b, &sethi_i8(8, (IO_WRCMD_CMD >> 16) as u8)));
assert!(!contains3(b, &iord(4, 8)));
```

`contains3`/`contains4` scan all 128 bytes, so a future edit that reintroduces
the read anywhere in the image — reachable or not — fails `./arroyo check`.

**`POKE_A_BYTES`** — the same skeleton, +7 bytes, with the `$r8` setup at
`0x10`/`0x14` and the ack replaced by `iord $r4, I[$r8]` → `iowr I[$r2], $r4`,
so `CC_SCRATCH[1]` carries **the word the falcon read out of `0x409504`**, not a
constant. Asserted to occur exactly once:

```rust
let mut n = 0; /* … scan … */ assert!(n == 1);
```

**The host block** sits immediately above the FECS access-ledger print and the
existing terminal `fecs_write(bar0, 0x409504, 0)`. Everything below the
ordering-contract banner is unchanged.

## 3. The seven defects in the salvage patch

The salvage patch (`docs/dev/GEMINI/salvage/kepler-echo-poke-split.patch`) does
not apply to this tree — it was cut against a parent whose `ucode-echo` verdict
was `ack != MB_SEED`, where the tree's is `ack == 1`. Its two byte arrays were
kept verbatim; everything around them was transcribed by hand and fixed.

**1 — FECS base.** The salvage POKE block wrote raw BAR0: `0x104`, `0x180`,
`0x184`, `0x800`, `0x804`, `0x100`, `0x044`, `0x040`. With no `0x409000` those
land in the PMC master-control block — ~66 wild MMIO writes into the GPU's
master control at boot — and because they bypass `fecs_read`/`fecs_write` the
FECS access ledger never counts one of them. Every access now goes through
`fecs_write/fecs_read(bar0, pbase + …)` with `pbase = 0x409000`.

**2 — CPUCTL/BOOTVEC swapped.** The salvage block treated `0x104` as CPUCTL and
`0x100` as BOOTVEC, so START_TRIGGER went into the wrong register. The map is
`CPUCTL = 0x100`, `BOOTVEC = 0x104` — what the ECHO block has had on the
s37-metal-proven path all along.

**3 — IMEMT tag and page padding.** The salvage upload wrote IMEMC and IMEMD
for 32 words and stopped. The code TLB marks a page usable only when the last
word of the `0x40`-word page is written, and the tag must match `BOOTVEC = 0`.
The block now writes IMEMC (AINCW), IMEMT tag 0, the 32 image words, then pads
to `IMEM_PAGE_WORDS`.

Any one of those three alone prevents the falcon from executing.

**4 — the verdict was too wide.** `if ack != MB_SEED` with only `0xBADF0000`
carved out printed SUCCESS for `0xFFFFFFFF` (bus float), `0xBAD0BA20` and
`0x00000000`. There is now one predicate, `classify_fecs_word()`, built from
the one this file already used for the runlist base
(`(x >> 16) == 0xBADF || (x >> 16) == 0xBAD0`) plus the two the "absent?" tests
already knew about:

```rust
POISON  (x >> 16) == 0xBADF || (x >> 16) == 0xBAD0
ABSENT  x == 0xFFFFFFFF
ZERO    x == 0
VALUE   otherwise
```

`SUCCESS` is printed only for `VALUE`. A poison read prints `POISON`.

**5 — `FECS_504_READ_TOUCHED` on the falcon-side read.** The ledger is
instrumented inside `fecs_read`/`fecs_write`, which see **host** accesses only.
A falcon `iord` is invisible to it, so on exactly the boot where the falcon
touched `0x409504` first, the ledger would have printed
`504_read_touched=false`. The POKE block sets the flag itself, before arming
the core.

> An instrument's silence is evidence only if the instrument can execute in the
> state it reports on.

A watcher that certifies the event it cannot see is worse than no watcher.

**6 — the three deleted guards are back, stronger.**

- Branch displacements resolve by arithmetic, not by byte literal with the
  target in a comment: `assert!(bra_target(b, 0x3b) == 0x69)`, where
  `bra_target(b, at) = at + b[at+2] as i8`. All six branches across both images.
- Port immediates go through `regs::falcon_io()` — the guard that exists
  specifically to catch a raw-host-offset listing, which is exactly what defect
  1 was on the host side.
- The second `falcon_io` the salvage patch added inside `mod ucode` is **not**
  transcribed. It shadowed `regs::falcon_io` and dropped its `& 0xffc` mask.

**7 — the smaller ones, all real.**

- `mailbox0={:08X}` printed `ack`, which is `CC_SCRATCH[1]`. It now prints
  `mb0`, and the poke's read-back is labelled `wrcmd_cmd=`.
- The POKE host poll spun to `ECHO_BOUND` with no `spin_loop()` — 1,048,576
  host MMIO reads, roughly 1 s of boot, from reusing a *Falcon instruction*
  budget as a host read count. There is now `HOST_ACK_ITERS = 100_000`
  (matching the echo leg) and a `spin_loop()`, and `ECHO_BOUND`'s doc comment
  says in as many words that it is not a host read count.
- `#[rustfmt::skip]` is back on both arrays — without it `cargo fmt` reflows
  them and destroys the one-instruction-per-line layout the whole review method
  depends on.
- `#[cfg(test)] mod tests` is deleted. See §5.

## 4. One defect found while fixing the seven: `PHASE_A_BOUND`

`PHASE_A_BOUND` was `u8 = 0xBD`, and the host compared
`phase == phase_bound as u32`. envydis disassembles the instruction that
produces it as:

```
00000057: f0 07 bd              mov $r0 -0x43
```

The I8 immediate is **signed**. `$r0` — and therefore MAILBOX1 — holds
`0xFFFFFFBD`, never `0xBD`. The exit-by-bound branch of the `ctx-echo` verdict
could not fire, on any boot, ever: it compared a sign-extended read against a
truncated constant. It is now `u32 = 0xFFFF_FFBD`; the assertion still checks
the image byte via `PHASE_A_BOUND as u8`.

This is the same class of failure as the four malformed instructions: a witness
that cannot report the state it is watching for. It was not in the brief's list
of seven; envydis found it.

## 5. `mod tests`, deleted rather than repaired

`#[cfg(test)] mod tests` did not compile: it called `pack92` (only `pack128`
exists), asserted against a 92-byte buffer, and pinned offsets that had moved.
It survived in that state because nothing runs `cargo test` on this `no_std`
kernel crate — `./arroyo check` is the gate, and `#[cfg(test)]` code is
invisible to it.

The coverage did not move to nowhere. The `const _: () = { … }` blocks pin all
128 bytes of **both** images, contiguously, padding included — strictly more
than the old tests sampled — and const evaluation **is** performed by the gate.
A test that cannot run is a comment that lies about being a test.

## 6. Verification — envydis, not comments

`envydis` built from the in-repo `envytools`:

```
cmake <repo>/envytools -DCMAKE_POLICY_VERSION_MINIMUM=3.5 && make envydis
```

Both arrays were extracted **from `kepler.rs`** by a script that strips `//`
comments and takes every `0x..` byte literal out of the
`pub const <NAME>: [u8; 128] = [ … ];` item — 128 bytes each, asserted — and
disassembled as falcon `fuc4`. The listings below are that tool's output
verbatim (ANSI colour stripped), not a transcription.

Verdict:

- **Zero unknown instructions** in either executable region — ECHO `0x00`–`0x66`,
  POKE `0x00`–`0x70`. Every line decodes.
- **All six branch displacements resolve** to a listed address, and envydis
  agrees with the const assertions:
  ECHO `0x34 → 0x5f`, `0x3a → 0x4e`, `0x54 → 0x25`;
  POKE `0x3b → 0x69`, `0x41 → 0x58`, `0x5e → 0x2c`.
  envydis marks every one of those six targets `B` (branch target), and marks
  no other address `B` — so there are no branch targets the assertions missed.
- **Every port immediate is `falcon_io()` of its host offset.** envydis prints
  the *resolved* register value for `sethi`, which reads it back for free:
  `$r1 = 0x0 | 0x20000 = 0x20000 = falcon_io(0x800)`;
  `$r2 = 0x100 | 0x20000 = 0x20100 = falcon_io(0x804)`;
  `$r6 = 0x1000 = falcon_io(0x040)`; `$r7 = 0x1100 = falcon_io(0x044)`;
  and in POKE `$r8 = 0x4100 | 0x10000 = 0x14100 = falcon_io(0x504)`.
- **Assertion coverage has zero unpinned holes.** Both const blocks step
  contiguously — each offset is the previous offset plus that instruction's
  width — from `0x00` to the first padding byte, and `zero_tail()` closes the
  rest to 128. Compare the offsets in the blocks against the addresses in the
  listings below: they are the same sequence.
- The trailing `st b8 D[$r0] $r0` lines are the disassembler decoding padding
  zeros. They are not reachable: every path through either image ends in
  `exit`, all of which lie before the padding. ECHO's last line shows
  `00 ?? ??  [incomplete]` — that is envydis running off the end of the
  128-byte buffer at `0x7f`, not a byte in the image.

### 6.1 `ECHO_A_BYTES` — full disassembly

```
00000000: f0 17 00              mov $r1 0x0
00000003: f0 13 02              sethi $r1 0x20000
00000006: f1 27 00 01           mov $r2 0x100
0000000a: f0 23 02              sethi $r2 0x20000
0000000d: f0 37 01              mov $r3 0x1
00000010: f1 67 00 10           mov $r6 0x1000
00000014: f1 77 00 11           mov $r7 0x1100
00000018: f0 57 00              mov $r5 0x0
0000001b: f1 53 10 00           sethi $r5 0x100000
0000001f: f0 07 01              mov $r0 0x1
00000022: d0 70 00              iowr I[$r7] $r0
00000025: cf 14 00            B iord $r4 I[$r1]
00000028: d0 64 00              iowr I[$r6] $r4
0000002b: f0 07 02              mov $r0 0x2
0000002e: d0 70 00              iowr I[$r7] $r0
00000031: b0 44 02              cmpu b32 $r4 0x2
00000034: f4 0b 2b              bra e 0x5f
00000037: b0 44 01              cmpu b32 $r4 0x1
0000003a: f4 1b 14              bra ne 0x4e
0000003d: f0 07 03              mov $r0 0x3
00000040: d0 70 00              iowr I[$r7] $r0
00000043: d0 23 00              iowr I[$r2] $r3
00000046: f0 07 04              mov $r0 0x4
00000049: d0 70 00              iowr I[$r7] $r0
0000004c: f8 02                 exit
0000004e: b6 52 01            B sub b32 $r5 0x1
00000051: b0 54 00              cmpu b32 $r5 0x0
00000054: f4 1b d1              bra ne 0x25
00000057: f0 07 bd              mov $r0 -0x43
0000005a: d0 70 00              iowr I[$r7] $r0
0000005d: f8 02                 exit
0000005f: f0 07 04            B mov $r0 0x4
00000062: d0 70 00              iowr I[$r7] $r0
00000065: f8 02                 exit
00000067: 00 00 00              st b8 D[$r0] $r0
0000006a: 00 00 00              st b8 D[$r0] $r0
0000006d: 00 00 00              st b8 D[$r0] $r0
00000070: 00 00 00              st b8 D[$r0] $r0
00000073: 00 00 00              st b8 D[$r0] $r0
00000076: 00 00 00              st b8 D[$r0] $r0
00000079: 00 00 00              st b8 D[$r0] $r0
0000007c: 00 00 00              st b8 D[$r0] $r0
0000007f: 00 ?? ??              st b8 D[$r0] $r0 [incomplete]
```

### 6.2 `POKE_A_BYTES` — full disassembly

```
00000000: f0 17 00              mov $r1 0x0
00000003: f0 13 02              sethi $r1 0x20000
00000006: f1 27 00 01           mov $r2 0x100
0000000a: f0 23 02              sethi $r2 0x20000
0000000d: f0 37 01              mov $r3 0x1
00000010: f1 87 00 41           mov $r8 0x4100
00000014: f0 83 01              sethi $r8 0x10000
00000017: f1 67 00 10           mov $r6 0x1000
0000001b: f1 77 00 11           mov $r7 0x1100
0000001f: f0 57 00              mov $r5 0x0
00000022: f1 53 10 00           sethi $r5 0x100000
00000026: f0 07 01              mov $r0 0x1
00000029: d0 70 00              iowr I[$r7] $r0
0000002c: cf 14 00            B iord $r4 I[$r1]
0000002f: d0 64 00              iowr I[$r6] $r4
00000032: f0 07 02              mov $r0 0x2
00000035: d0 70 00              iowr I[$r7] $r0
00000038: b0 44 02              cmpu b32 $r4 0x2
0000003b: f4 0b 2e              bra e 0x69
0000003e: b0 44 01              cmpu b32 $r4 0x1
00000041: f4 1b 17              bra ne 0x58
00000044: f0 07 03              mov $r0 0x3
00000047: d0 70 00              iowr I[$r7] $r0
0000004a: cf 84 00              iord $r4 I[$r8]
0000004d: d0 24 00              iowr I[$r2] $r4
00000050: f0 07 04              mov $r0 0x4
00000053: d0 70 00              iowr I[$r7] $r0
00000056: f8 02                 exit
00000058: b6 52 01            B sub b32 $r5 0x1
0000005b: b0 54 00              cmpu b32 $r5 0x0
0000005e: f4 1b ce              bra ne 0x2c
00000061: f0 07 bd              mov $r0 -0x43
00000064: d0 70 00              iowr I[$r7] $r0
00000067: f8 02                 exit
00000069: f0 07 04            B mov $r0 0x4
0000006c: d0 70 00              iowr I[$r7] $r0
0000006f: f8 02                 exit
00000071: 00 00 00              st b8 D[$r0] $r0
00000074: 00 00 00              st b8 D[$r0] $r0
00000077: 00 00 00              st b8 D[$r0] $r0
0000007a: 00 00 00              st b8 D[$r0] $r0
0000007d: 00 00 00              st b8 D[$r0] $r0
```

## 7. Gate

Run from `unaos/` in the worktree, with the knobs that actually compile
`kepler.rs` — a bare `./arroyo check` does not build this file at all:

```
UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 UNAOS_IVB=1 UNAOS_SMC=1 ./arroyo check
```

```
⚡ kernel features: ehcihid,smc,smolnet,nvidia-kepler,nvidia-kepler-takeover,nvidia-kepler-fifo,intel-ivb,unaos_ivb
✅ x86_64 OK
✅ aarch64 OK
```

The banner lists `nvidia-kepler`, so the const-assertion blocks in `mod ucode`
were evaluated. No new warnings are attributable to this change; the `kepler.rs`
warnings in the output (`unnecessary unsafe block` ×8 at 793–880, `value
assigned to bar1_base is never read` at 610) are all pre-existing and outside
the changed regions.

No QEMU suite was run. Metal is the verdict; QEMU has no FECS falcon.

## 8. What this arc did NOT do

- **Nothing here ran on hardware.** Every claim above is about bytes, types and
  compile-time assertions. Whether the falcon executes `POKE_A_BYTES`, and what
  `0x409504` returns to an `iord`, is unanswered until the next sitting.
- **The runlist beacon overwrite is untouched.** Recorded in
  `FINDINGS-kepler-poke-terminal.md`; it is a separate arc.
- **POKE has no distinct phase magics.** Both images stamp `0x01`–`0x04`, so
  MAILBOX1 alone does not name which image ran — a weakening of the pull-25
  distinct-magic discipline. The byte arrays were kept verbatim as instructed,
  so this was recorded rather than changed. It is mitigated in practice: the
  two legs run at different points and print different labels (`ctx-echo` vs
  `ctx-poke`).
- **No file outside `kepler.rs`** and this lane's three docs was touched.
  `main.rs`, `shell.rs`, `arroyo`, `builder/` and `interrupts.rs` are
  unmodified.
- **Nothing was pushed, merged or rebased.**

## 9. What to look for in the next capture

```
:: kepler: ucode-echo SUCCESS h2h3=on mb0=00000001 ::
```
— the echo test now passes or fails without having touched `0x409504`. Anything
printed after this line is an observation of an *unpoisoned* unit for the first
time since the read was introduced. That alone changes the value of the sweep,
`recon-post`, and the ledger.

```
:: kepler: ctx-poke img=POKE ack=XXXXXXXX mb0=XXXXXXXX phase=XXXXXXXX iters=N class=... ::
```

- `class=VALUE` — the falcon read `0x409504` and got a real word. The host-side
  fault would then be a property of the host access path, not of the register.
  That is a new fact, and the first thing `0x409504` has ever told anyone.
- `class=POISON` — the fault is in the unit and reachable from both sides.
- `phase=FFFFFFBD` — exit by bound: the command never reached the falcon, the
  `iord` never executed, and **nothing** is claimed about `0x409504`. Note that
  `504_read_touched=true` will still be printed in the ledger on that boot,
  because the host block sets it before arming the core; the flag means "this
  boot armed an image that reads it", and `phase` is what says whether the read
  happened. Read the two together.
- `ucode-poke ABORT verify-mismatch` — IMEM read-back disagreed with what was
  written; the core was never armed and `0x409504` was not read.
