# RXMERGE — A37: one owner for serial RX (exactly-once, in-order)

Ledger row: `docs/dev/OS/orin-ledger.md` A37 (the defect), A16 (rung 2, the second source it came
from), A22 (rung 1, the probe). Executor SERIALRX-DEDUP, seat orin 16, 2026-09-06, base `37c78ad7`
(hw-jetson tip). No new knob — the fix rides `tcurx` (`UNAOS_TCURX=1`), DEFAULT OFF. Prior rungs:
[`../orin14/TCURX-DESIGN.md`](../orin14/TCURX-DESIGN.md), [`../orin15/TCURX2.md`](../orin15/TCURX2.md).

## 1. The mechanism, read off render7

Flight log `~/unaos-bench/scratch/orin16/render7-boot1.log` (bench-side, unversioned), injector log
`render7-paced-inject.out` beside it proving five bytes were sent on each leg. Line numbers are that
log's; read with `awk`, never `grep`.

**BURST `tste` + CR.** UARTC's RBR delivered `s`, `t`, CR (:1347-1349); the mailbox delivered `t`,
`e` SIXTEEN LINES LATER (:1363-1366), after the shell had already run the CR:

```
1347  :: tegra: JD2 — KEY 's' ::
1348  :: tegra: JD2 — KEY 't' ::
1349  :: tegra: JD2 — KEY 0x0d ::
1351  :: [midden] cmd="st" -> TerminalError len=44 ::
1363  [tcurx] took=0x74 't' left=1 word=0x81000065 <- raw=0x82006574 … took-total=1
1364  [tcurx] took=0x65 'e' left=0 word=0x00000000 <- raw=0x81000065 … took-total=2
1365  :: tegra: JD2 — KEY 't' ::
1366  :: tegra: JD2 — KEY 'e' ::
1367  [serialrx] rx=5 (+5) … mbox=2
```

Five in, five out — **exactly-once held on this leg** — but the delivered order was `s t CR t e`.
The census confirms the split: `rx=5 (+5) … mbox=2`, i.e. 3 from UARTC and 2 from the mailbox.

**PACED, the same five bytes, 50 ms apart.** The mailbox carried ALL FIVE, in order — `t`,`s`,`t`,`e`
(:1498-1507) and CR (:1521) — and UARTC ALSO delivered the CR (:1508, with no `[tcurx] took=` ahead
of it, which is what identifies its source):

```
1498-1507  [tcurx] took=0x74/0x73/0x74/0x65  (took-total 3,4,5,6) → KEY 't','s','t','e'
1508       :: tegra: JD2 — KEY 0x0d ::                ← UARTC RBR: no [tcurx] line precedes it
1510       :: [midden] cmd="tetste" -> TerminalError ::
1521       [tcurx] took=0x0d '.' … took-total=7       ← the SAME CR, again, from the mailbox
1522       :: tegra: JD2 — KEY 0x0d ::
1527       [serialrx] rx=11 (+6) … mbox=7
```

`+6` keys for five injected bytes. **The same byte came down both transports.**

**The mechanism in two sentences.** UARTC is the SPE/TCU's combined-UART port (A16): the SPE reads
its RBR and forwards console RX into the HSP shared mailbox, which is the CCPLEX's real console-input
contract, while our `serialrx::drain` polls the very same RBR — two masters on one RX FIFO with no
shared sequence. Each RBR read normally pops an entry so the two readers split the stream (burst: 3
to us, 2 to the SPE), but the pop is not atomic across masters, so an overlapping pair of reads can
both retire the same entry (the paced CR); and because the RBR is delivered in the pass that reads it
while the mailbox arrives whenever the SPE posts and we next drain, the interleave at `Event::Key`
is arbitrary.

Answers to the three questions the arc was briefed with:

| question | answer, from the wire |
|---|---|
| does the mailbox hold exactly the bytes UARTC loses? | on the BURST leg yes — `t`,`e` are precisely the two UARTC missed, and the union is the injected multiset exactly once. Not a general law: on the PACED leg the mailbox held all five |
| does UARTC's RBR ever see a byte the mailbox also carries? | **YES** — the paced CR, delivered at :1508 (UARTC) and :1521-1522 (mailbox). That is the duplicate |
| does the SPE forward all RX, or only overflow? | **all of it that we do not steal first.** Paced, with our poll losing every race, the mailbox carried 5/5 in order. Burst, with our poll winning three, the mailbox carried the remaining 2. The mailbox's short share is caused by the competing reader, not by a forward-only-on-overflow policy |

## 2. Why a merge queue cannot fix this, and one owner can

A merge "keyed by arrival" is **what the code already did**: both sources pushed into one PAL queue
in observation order, and that is what produced `cmd="st"`. There is no ordering tag on the wire —
the TCU word carries a byte COUNT, not a sequence number — so no consumer can reconstruct send order
from two unsequenced transports. Nor can the duplicate be filtered by value: a human typing `tt`
sends two identical bytes, and a value filter would silently eat the second keystroke, which is the
same class of defect as the loss we are repairing. **Exactly-once and in-order therefore require
exactly one reader.**

Which one: **the mailbox**, on the paced leg's evidence — it carried 5 of 5 in order, unaided. The
burst leg's mailbox share was short precisely because our RBR poll stole three bytes first; remove
the thief and the SPE has the whole stream to forward.

**Parking means NOT READING.** A read-and-discard would be strictly worse than the bug: the RBR read
is what pops the byte away from the SPE, so discarding it destroys the byte outright. `drain()` skips
the poll entirely and counts the pass in `parked=`.

**The RBR poll is kept (R19).** It is the source whenever the mailbox never armed — no DTB
resolution, or a board where the TCU is not the console — and it is unchanged in the `tcurx`-off
image.

## 3. What changed

| file | site | what |
|---|---|---|
| `unaos/crates/kernel/src/arch/aarch64/hsp_tegra.rs` | tail append, `#[cfg(feature = "tcurx")]` | `rx_mbox_armed()` — the arbitration's input: did rung 1 resolve the RX mailbox word from the live DTB |
| `unaos/crates/kernel/src/arch/aarch64/serial.rs` | `drain()` | the RBR `while` loop is wrapped in `if uartc_owns_rbr()`, with an `else` that counts `PARKED`; both loops now hand bytes to the single intake `deliver(src, byte)` |
| same | `census()` | one added statement, `#[cfg(feature = "tcurx")] rxmerge_census();` |
| same | tail of `pub mod serialrx` | the RXMERGE block: the rule as `const fn`s, the `const _` gate, the counters, `uartc_owns_rbr`, `deliver`, `rxmerge_census`, and `seed_lsr_parked` (keeps A16's LSR/IIR witness alive on a parked port — §5) |

`main.rs`, `mod.rs`, `Cargo.toml` and `arroyo` are **untouched** — no new knob, no new leg, no
GATE-KNOB churn. Everything added is inside the `tcurx`-gated part of a module that is itself
`#[cfg(all(feature = "tegra", feature = "orinrx"))]` and sits at the file's tail, so knob-off builds
— the Pi's `kernel8.img` included — are byte-identical (gate 3 below).

## 4. The rule, and the self-test

QEMU models no Tegra234, so there is no emulated wire to inject into. The rule is therefore stated as
`const fn`s and checked by **const evaluation on every `./arroyo check`**, on the `arm-tegra-tcurx`
leg where the code lives:

```rust
pub const fn polls_uartc(policy: u8) -> bool;                                   // parking = a READ ban
pub const fn policy_for(mbox_armed: bool) -> u8;                                // the arbitration
pub const fn is_handoff(prev_src, src, have_prev) -> bool;                      // reorder= signature
pub const fn is_xdup(prev_src, prev_byte, src, byte, have_prev) -> bool;        // dup= signature
pub const fn replay<const N: usize>(policy, &[(src, byte); N]) -> (u32, u32, u32);
```

`#[cfg(test)] mod tests` was rejected deliberately: nothing runs `cargo test` on this `no_std` kernel
crate and `./arroyo check` cannot see it — the documented reason `gui_watchdog.rs` and
`drivers/gpu/kepler.rs` both removed theirs. A `const _` block is strictly stronger: a regression in
the ordering/dedup rule is a **build failure of the gate command itself**.

The asserted cases are render7's two legs as actually recorded, plus the repeat case:

| feed | policy | `(delivered, reorder, dup)` | what it fixes in place |
|---|---|---|---|
| BURST `U:s U:t U:CR M:t M:e` | both | `(5, 1, 0)` | render7's reorder, with exactly-once intact |
| PACED `M:t M:s M:t M:e U:CR M:CR` | both | `(6, 2, 1)` | render7's double-CR: six keys for five bytes |
| PACED, same wire | mbox-only | `(5, 0, 0)` | **the claim** — five injected bytes, in order, once each |
| BURST, same wire | mbox-only | `(2, 0, 0)` | the three bytes we no longer steal are never popped by us; what is delivered is single-source and ordered |
| `M:t M:t` | mbox-only | `(2, 0, 0)` | a legitimate same-source repeat is NOT a duplicate — `tt` keeps both bytes |

**The gate was proven able to fire**, not merely to pass: flipping the third row's expectation from
`(5, 0, 0)` to `(6, 0, 0)` turns the leg red with
`error[E0080]: evaluation panicked: assertion failed: matches!(replay(POLICY_MBOX_ONLY, &PACED), (6, 0, 0))`
at `serial.rs:1012`, and reverting it green again.

## 5. On the wire

One-shot, at the drain pass where the mailbox arms:

```
[rxmerge] policy=mbox-only armed=1 uartc-rbr=parked -> A37: one owner, one ordered stream (…)
```

Per byte delivered, from either source — **this is the line the next flight is scored on**:

```
[rxmerge] src=mbox seq=4 byte=0x0d '.' policy=mbox-only dup=0 reorder=0
```

On the census cadence, beside `[serialrx] rx=`:

```
[rxmerge] census policy=mbox-only seq=5 uartc=0 mbox=5 dup=0 reorder=0 parked=… -> SINGLE-SOURCE (…)
```

`dup=` counts a cross-source value repeat (render7's double CR); `reorder=` counts a cross-source
handoff in the delivered stream (render7's `s t CR` → `t e`). Both are **detectors, never filters** —
no byte is ever dropped on their account, and under `policy=mbox-only` neither can be non-zero,
which is the whole claim. Non-zero on a flight is a finding, not a regression.

**EXPECTED SIDE EFFECTS, so they are not mis-scored as regressions.** With the RBR parked, no poll
reaches the port, so `[serialrx]` prints `polls=0` for the whole boot: score RX by `[rxmerge] census`
and `[tcurx] took=`, and read `parked=` as the drain-liveness counter that replaces `polls=`.

A16's one-shot `[serialrx] lsr=… iir=… fifo=…` witness is **kept**, because a witness that silently
stops appearing is the exact failure the serial-transport law forbids. `note_lsr` is fed from inside
`read_byte`, which is what parking stops calling, so the parked branch takes the status word
directly, once per boot, behind the same DARKWIN-GUARD `read_byte` opens with. **An LSR read returns
the line state and does not pop the RBR**, so unlike a data read it cannot steal a byte from the SPE
— the same one-shot discipline and accepted side-effect class as the IIR read PANEL4 L3 sanctioned.
One consequence for the scorer: `ovrf=` counts POLLS that saw the overrun flag, and there is now
exactly one such poll per boot, so **`ovrf=` is at most 1** under `policy=mbox-only`.

## 6. The flight question for the next render

Same knob line as render7 (no new knob):

```
UNAOS_TEGRA=1 UNAOS_TEGRA_EL0=1 UNAOS_WITNESS=1 UNAOS_ORINRENDER=1 UNAOS_DESKCASCADE=1 \
UNAOS_ORINRX=1 UNAOS_HOLOCRON=1 UNAOS_ORINCLICK=1 UNAOS_TCUPROBE=1 UNAOS_TCURX=1 ./arroyo esp-jetson
```

Inject `tste\r` twice with `~/unaos-bench/tools/inject-paced.sh` — BURST (0 ms) then PACED (50 ms).

**PASS = five `KEY` lines per leg in the injected order (`t`,`s`,`t`,`e`,CR), `:: [midden]
cmd="tste"` on both, and `[rxmerge] census … dup=0 reorder=0 uartc=0` with `seq=` at 10 after both
legs.**

| what the flight shows | reading |
|---|---|
| 5/5 in order on both legs, `dup=0 reorder=0 uartc=0` | A37 fixed. The mailbox is the console, and one owner is all that was missing |
| 5/5 in order, but `uartc>0` | the arbitration did not engage before the injection — check the one-shot `[rxmerge] policy=` line and `[tcu] hsp …` |
| fewer than 5 per leg, `mbox=` short, `parked=` climbing | the SPE does NOT forward everything: the burst leg's short mailbox share was not caused by our theft after all. A16's alternative (move the console to a UART the SPE does not own) comes back — the RBR path is still in the tree for exactly this |
| `dup>0` or `reorder>0` | two readers are still both delivering; the `[rxmerge] src=` lines name which transport carried each byte |
| no `[rxmerge]` line at all | `tcurx` did not reach the artifact: check the banner and `strings kernel.elf` |

## 7. Gates (this tree, 2026-09-06, base `37c78ad7`)

| # | gate | result |
|---|---|---|
| 1 | `cd unaos && ./arroyo check` | **exit 0** — `✅ x86_64 OK`, `✅ aarch64 OK`, all cfg legs incl. `✅ arm-tegra-tcurx`, `GATE-KNOB: OK — 158 features, 157 named, 0 phantom, 0 dead`, `GATE-LEDGER: OK` |
| 2 | `./arroyo test-arm 60` | **exit 0** — `✅ aarch64 test complete` |
| 3 | `./arroyo kernel8` before vs after | **IDENTICAL** — `target/pi_baremetal/kernel8.img` sha256 `8ff7c1d1f4e8938d9a29df4a094ecc1fe01684350adeef8a577b13c5eb89dc13` at `37c78ad7` and with every edit applied. The baseline was taken by snapshotting the diff to `~/unaos-bench/scratch/orin16/serialrx-dedup/rxmerge.patch`, reverse-applying it, building, and re-applying — the stash is forbidden in this repo (LAWS) |
| 4 | armed `esp-jetson` | **exit 0**; banner `⚡ kernel features (jetson): …,orinrx,tcuprobe,tcurx,deskcascade`; in the ARTIFACT, `grep -a -c` on `target/aarch64_esp/kernel.elf`: `[rxmerge] src=` 1, `[rxmerge] census` 1, `[rxmerge] policy=` 1, `SINGLE-SOURCE` 1, `SPLIT-SOURCE` 1, `[tcurx] took` 1 |
| 5 | the const gate is falsifiable | proven — see §4 |

Media was NOT staged and no card was written (brief).

## 8. Files
- `unaos/crates/kernel/src/arch/aarch64/serial.rs` (`drain`, `census`, the RXMERGE tail block),
  `unaos/crates/kernel/src/arch/aarch64/hsp_tegra.rs` (tail append `rx_mbox_armed`).
- Facts come from the DTB, the public DT bindings and BSD-licensed edk2-nvidia only — no GPL NVIDIA
  driver was read (orin-ledger D3, CLEAN_ROOM_POLICY §6). Sources: `../orin14/TCURX-DESIGN.md` §2.
