# PROPOSAL — split ECHO from POKE; the 0x409504 read becomes terminal

Lane: `kepler` · branch `wt/kepler-poke-x86` · file
`unaos/crates/kernel/src/drivers/gpu/kepler.rs` (only).

## 1. The defect this arc removes

`ECHO_A_BYTES` — the image whose whole purpose is to prove *that the FECS falcon
executes at all* — carries, inside the `cmd == 1` arm:

```
mov   $r8, 0x4100
sethi $r8, 0x01        ; $r8 = 0x14100 = falcon_io(0x504)
...
iord  $r4, I[$r8]      ; read WRCMD_CMD, 0x409504
```

That image runs **mid-sequence**, from the `ucode-echo` loop, roughly 400 lines
of boot before the terminal poke. The poison law
(`docs/dev/OS/08_VIDEO/falcon_microcode_spec.md` §5.4, and its §10 heading
"The terminal poke — 0x409504, once, last") says the first access to `0x409504`
faults and wedges every subsequent read in the FECS unit for the rest of the
boot. So the echo test poisons the unit it is testing, and every FECS
observation printed after it is suspect.

Reading `0x409504` from the falcon side, where the *host* side faults, is a
legitimate experiment — it asks a question the host cannot ask. It simply has to
be the **last** thing the kepler leg does.

## 2. What lands

Two images, byte-exact, both 128 bytes:

| Image | Purpose | `$r8` / `0x504` |
|---|---|---|
| `ECHO_A_BYTES` | falcon-execution test: command in, ACK out, phase stamps | **absent, mechanically asserted absent** |
| `POKE_A_BYTES` | the falcon-side `iord` of `0x409504` | present; runs **once**, at the terminal phase |

`ECHO_A_BYTES` drops the `$r8` setup and the `iord I[$r8]`, and acks with
`iowr I[$r2], $r3` (`$r3 = 1`). It keeps the s37-proven prologue word for word,
the split observable (MAILBOX0 = the word read), the phase stamps in MAILBOX1,
and the bounded down-counting poll loop.

`POKE_A_BYTES` is the same skeleton with the `$r8` setup restored and the ack
replaced by `iord $r4, I[$r8]` → `iowr I[$r2], $r4`, so `CC_SCRATCH[1]` carries
**the value the falcon read out of `0x409504`**, not a constant.

The POKE host block is inserted immediately above the FECS access-ledger print
and the existing terminal `fecs_write(bar0, 0x409504, 0)`. Nothing is added
below the ordering-contract banner.

## 3. Starting point, and the seven defects fixed on top of it

The arc starts from `docs/dev/GEMINI/salvage/kepler-echo-poke-split.patch` — the
previous session's split, abandoned uncommitted. Its two byte arrays are kept
verbatim; everything around them is rebuilt. (The patch does **not** apply to
this tree — it was cut against a parent whose `ucode-echo` verdict was
`ack != MB_SEED`; the tree's verdict is now `ack == 1`. It is transcribed by
hand, hunk by hunk.)

1. **FECS base.** The salvage POKE host block addressed raw BAR0 — `0x104`,
   `0x180`, `0x184`, `0x800`, `0x804`, `0x100`, `0x044`, `0x040` — which is the
   PMC master-control block, not the FECS falcon, and bypasses the access
   ledger. Every access becomes `fecs_write/fecs_read(bar0, base + …)` with
   `base = 0x409000`, as the ECHO block already does.
2. **CPUCTL / BOOTVEC.** `CPUCTL = 0x100`, `BOOTVEC = 0x104` (the ECHO block's
   s37-metal-proven order). The salvage block had them swapped.
3. **IMEMT tag and page padding.** The upload writes IMEMC (AINCW), IMEMT tag 0
   to match `BOOTVEC = 0`, the 32 image words, and then pads to
   `IMEM_PAGE_WORDS = 0x40` — the code TLB marks a page usable only when the
   last word of the page is written.
4. **The verdict.** A single classifier, reusing this file's own existing
   predicate (`(x >> 16) == 0xBADF || (x >> 16) == 0xBAD0`, plus `0xFFFFFFFF`
   and `0`), decides what the read-back means. **A poison read prints POISON,
   never SUCCESS**; a floating bus prints ABSENT; an all-zero word prints ZERO.
   SUCCESS is reserved for a word that is none of those.
5. **`FECS_504_READ_TOUCHED`.** The ledger only instruments host
   `fecs_read`/`fecs_write`; a falcon `iord` is invisible to it. The POKE block
   sets the flag itself, before arming the falcon, so the ledger cannot certify
   an untouched `0x409504` on the one boot where the falcon touched it first.
   *An instrument's silence is evidence only if the instrument can execute in
   the state it reports on.*
6. **The three deleted guards return.** Branch displacements asserted by
   arithmetic (`0x3b + slice3(..)[2] as i8 == 0x69`), not by byte literal with
   the target in a comment. Port immediates derived through `regs::falcon_io()`.
   The second `falcon_io` the salvage patch added inside `mod ucode` — which
   shadowed `regs::falcon_io` and dropped its `& 0xffc` mask — is not
   transcribed.
7. **Smaller.** `mailbox0={:08X}` stops printing `ack` (which is
   `CC_SCRATCH[1]`, not MAILBOX0). The POKE host poll gets its own host-side
   bound with `spin_loop()`, instead of reusing `ECHO_BOUND` — a *falcon
   instruction* budget — as a host MMIO read count. `#[rustfmt::skip]` stays on
   both arrays. `mod tests` is resolved (see §5).

## 4. Verification bar

`envydis`, built from the in-repo `envytools` (`cmake -DCMAKE_POLICY_VERSION_MINIMUM=3.5`,
gcc), run as falcon `fuc4` over **both** arrays extracted mechanically from
`kepler.rs` — not from this document. The bar:

- zero unknown instructions in each executable region;
- every branch displacement resolved by arithmetic to a listed address;
- every port immediate equal to `regs::falcon_io()` of its host offset;
- assertion coverage with zero unpinned holes across all 128 bytes of both
  arrays, padding included.

Both full listings go in the walkthrough.

## 5. `mod tests`

`#[cfg(test)] mod tests` does not compile: it calls `pack92` (only `pack128`
exists), asserts against a 92-byte buffer, and pins offsets that moved. It is
also unreachable — nothing in `arroyo` runs `cargo test` on the `no_std` kernel
crate, which is why a non-compiling module survived. The `const _: () = { … }`
blocks are strictly stronger (they pin all 128 bytes of both arrays, not a
sample) **and they are evaluated by `./arroyo check`, which is the gate**.
`mod tests` is therefore deleted, not repaired: a test that cannot run is a
comment that lies about being a test.

## 6. Gate

```
UNAOS_KEPLER=1 UNAOS_KEPLER_TAKEOVER=1 UNAOS_KEPLER_FIFO=1 UNAOS_IVB=1 UNAOS_SMC=1 ./arroyo check
```

both arches, with the `⚡ kernel features:` banner confirmed to list
`nvidia-kepler`. No QEMU suite is run; metal is the verdict.

## 7. Out of scope, recorded not touched

The runlist submitted with `LEN=3` contains beacon words — `kepler.rs` writes
three entries, then overwrites `runlist_off+0..+31` with
`0xBEAC0001..0xBEAC0008`, and nothing restores it before the submit. Separate
arc; it goes in `FINDINGS-kepler-poke-terminal.md`.

## 8. Expected wire output

```
:: kepler: ucode-poke pre CC_SCRATCH[0]=... CC_SCRATCH[1]=A5A50000 mb0=... mb1=... ::
:: kepler: ucode-poke uploaded words=32 padded=64 ::
:: kepler: ucode-poke start img=POKE ::
:: kepler: ctx-poke img=POKE ack=XXXXXXXX mb0=XXXXXXXX phase=XXXXXXXX class=... ::
:: kepler: ucode-poke {SUCCESS|POISON|ABSENT|ZERO|FAILURE|EXIT-BY-BOUND} img=POKE ... ::
:: kepler: fecs-ledger ... 504_read_touched=true ... ::
:: kepler: terminal-poke 0x409504 wr=0 (post: no further FECS reads this boot) ::
```

The interesting outcomes, in order of what they would settle:

- `class=VALUE` — the falcon read `0x409504` and got a real word. The host-side
  fault is a host-path property, not a register property. That is a new fact.
- `class=POISON` — the fault is in the unit, reachable from both sides.
- `phase=FFFFFFBD` — exit by bound; the command never reached the falcon and the
  read never happened. Nothing is claimed about `0x409504`.
