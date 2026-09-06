# S7 step 1, v2 — the render_service conversion re-cut against pi's body (`8131cd2d`)

Seat: orin 15, executor S7RECUT. Base: `8131cd2d` (= `origin/hw-pi4`), worktree branch
`exec-orin15-s7recut`. Design: [`../orin14/S7-CONVERGENCE.md`](../orin14/S7-CONVERGENCE.md) §3–§4 step 1.
v1 record: [`../orin14/S7-STEP1.md`](../orin14/S7-STEP1.md), patch
[`../orin14/S7-STEP1.patch`](../orin14/S7-STEP1.patch).

**Deliverables.** [`S7-STEP1-v2.patch`](S7-STEP1-v2.patch) — the conversion, applies on `8131cd2d`.
[`S7-STEP1-v2-tail.patch`](S7-STEP1-v2-tail.patch) — the 164-line tail block alone, applies on
`hw-jetson` `a05c2c8e`. [`S7-STEP1-v2-resolve.py`](S7-STEP1-v2-resolve.py) — the resolver that produced
v2 from the three-way merge, so the re-cut is reproducible rather than asserted.
Neither patch is merged anywhere: `main.rs` is rmbp's shared-kernel-core lane and
the pi byte-identity baseline moves, so the code stays a patch until the rmbp grant and the pi ack
are in hand. `arroyo`, `Cargo.toml` and every `video/` file are untouched.

## 1. What changed vs v1, and why

v1 was cut against `main.rs` at `33dc7811` on `hw-jetson` (patch base blob `8234c567`).
`git apply --check` of v1 at `8131cd2d` **fails at `main.rs:5357`** (exit 1, quoted in §4 gate 5).
pi's body differs from that base by five lines in the render region, four of which are WITNESSES
FOLDED ONTO EXISTING STATEMENTS — pi's line-neutrality discipline for `panic::Location` — and one of
which is a comment-text difference:

| pi line | pi's line | v1's base |
|---|---|---|
| `:5373` | `let t0 = …now_cycles(); #[cfg(livecon)] …console_live_service(); // … the arc `video/pidesk.rs`'s live-console ledger names …` | same statement, comment says `` `video/desktop_firmware.rs` `` |
| `:5374` | `s6_passes += 1; #[cfg(feature = "witness")] SHELLUP_RENDER_HB.fetch_add(1, …)` | `s6_passes += 1;` |
| `:5589` | `shell_id = id; #[cfg(feature = "witness")] SHELLUP_SHELL.store(id as u64, …)` | `shell_id = id;` |
| `:5613` | `shell_declined = true; #[cfg(feature = "witness")] SHELLUP_SHELL.store(SHELLUP_SHELL_DECLINED, …)` | `shell_declined = true;` |
| `:5686` | `…prio_witness(); #[cfg(feature = "witness")] …stk_probe("render:pass")` | `…prio_witness();` |

The hazard the re-cut exists to avoid: v1 REWRITES three of those five host statements while moving
the body into `render_pass<W>` (`s6_passes += 1` → `ChannelWait::wait`'s `self.passes += 1`;
`prio_witness()` → `ChannelWait::census`; the `let t0 = …` line is split). A hand-applied conversion
drops the appended witness with **no conflict marker**. So v2 was produced mechanically, not by hand:

```
$ git merge-file -p pi.rs before_v1.rs after_v1.rs      # ours = 8131cd2d, base = blob 8234c567,
                                                        # theirs = v1's result blob d2514064
   → 2 conflicts (main.rs:5369-5381 and :5671-5731)
```

Both conflicts are N→N (5↔5 and 29↔29) and both were resolved keeping v1's converted text plus pi's
witness/comment content (`~/unaos-bench/scratch/orin15/s7recut/resolve.py`, asserted line counts):

1. **`:5369-5373` (the wait head).** v1's five lines, with v1's livecon line replaced by **pi's own
   livecon line minus its `let t0 = …now_cycles();` host** — i.e. pi's comment wording
   (`` `video/pidesk.rs` ``) is preserved verbatim. The resolver asserts that pi's and v1's livecon
   comments differ in that one token and nothing else.
2. **`:5671-5699` (the census tail).** v1's prose block, verbatim.

The two witnesses whose host statements moved were then **folded onto those same host statements in
their new home**, in the appended tail block:

| witness | pre-S7 host | v2 host | v2 line |
|---|---|---|---|
| `SHELLUP_RENDER_HB.fetch_add(1, …)` | `s6_passes += 1;` (body) | `self.passes += 1;` (`ChannelWait::wait`) | `:9082` |
| `stk_probe("render:pass")` | `prio_witness();` (body) | `prio_witness();` (`ChannelWait::census`, verbatim move) | `:9120` |
| `SHELLUP_SHELL.store(id as u64, …)` | `shell_id = id;` | unchanged in the body | `:5589` |
| `SHELLUP_SHELL.store(SHELLUP_SHELL_DECLINED, …)` | `shell_declined = true;` | unchanged in the body | `:5613` |

Each append still goes **before the line's first `//`** (the folded statement precedes its own comment),
so pi's fold rule is preserved unchanged.

**One ordering note, stated rather than buried.** Pre-S7 the pass ran `now_cycles()` → livecon →
`s6_passes += 1` → HB. In v2, `ChannelWait::wait` runs `self.t0 = now_cycles()` → `self.passes += 1` →
HB and RETURNS, and the body then runs livecon. So the heartbeat bump and the livecon service call
swap order within the pass. Both fire exactly once per pass, the HB is a `Relaxed` counter read from
another core, and `console_live_service` neither reads nor writes it — no wire or cadence consequence.
The alternative (riding the HB on `let wake = w.wait();` in the body) would produce the SAME order, so
the swap is a consequence of the conversion, not of the placement choice.

**Flagged for pi 7, not changed.** pi's `:5373` comment names `` `video/pidesk.rs` ``, but this tree has
`unaos/crates/kernel/src/video/desktop_firmware.rs` and no `pidesk.rs` (`ls unaos/crates/kernel/src/video/`).
The jetson wording is the current one. v2 preserves pi's text verbatim because renaming it is outside
this brief; pi 7 should decide whether to fix it in a separate line-neutral edit.

**Also split out.** v1's tail hunk (`@@ -8624,3 +8624,167 @@ fn tegra_desk_cascade()`) no longer
applies on `hw-jetson` either — A19FIX `6f56eff8` appended past that anchor:

```
$ GIT_INDEX_FILE=… git read-tree a05c2c8e
$ GIT_INDEX_FILE=… git apply --check --cached S7-STEP1.patch
error: patch failed: unaos/crates/kernel/src/main.rs:8624
error: unaos/crates/kernel/src/main.rs: patch does not apply
EXIT=1
```

Note what that failure says: on `hw-jetson` the ten mid-file hunks still apply and ONLY the tail
anchor is stale. Hence deliverable 2 — the same 164 lines, re-anchored at `a05c2c8e`'s `@@ -8866,3 @@`.

**The tail fragment carries v1's block VERBATIM, without the two witness folds — deliberately.**
`SHELLUP_RENDER_HB` and the `[shellup]` statics are pi-only: `grep -c SHELLUP` on
`git show a05c2c8e:unaos/crates/kernel/src/main.rs` is **0** (`stk_probe` is 14, so that half would
resolve; the heartbeat would not). A tail fragment carrying the HB fold could not compile on
`hw-jetson`'s `arm-pi` leg. So the two blocks differ by exactly the two lines tabulated above, and
whichever tree the arc finally lands on decides which form is correct: a tree that carries pi's
SHELLUP block takes the v2 form, one that does not takes the fragment form. The delta is two lines
and it is written out here so nobody has to re-derive it.

## 2. Witness-count gate

The gate that matters is not "does the patch apply" but "are pi's four folded witnesses still there".
Command (the `stk_probe` literal's parentheses written as `.` so the shell/awk quoting is not part of
the claim; `grep -c` per pattern below is the exact form):

```
$ awk '/SHELLUP_RENDER_HB.fetch_add|SHELLUP_SHELL.store|stk_probe..render:pass../' <main.rs> | wc -l
```

| pattern | before (`8131cd2d`) | after (v2 applied) |
|---|---|---|
| `SHELLUP_RENDER_HB.fetch_add` | 1 | 1 |
| `SHELLUP_SHELL.store` | 2 | 2 |
| `stk_probe("render:pass")` | 1 | 1 |
| **combined awk** | **4** | **4** |

Equal. `grep -c '<pattern>' main.rs` gives the per-row numbers on both trees.

## 3. Hunk positions

`git diff` of the patched tree, all eleven hunks. The ten mid-file hunks are **N→N** (rmbp 12
condition d) and land at exactly the same line numbers as v1 — pi's five extra lines are all
fold-appends onto lines that already existed, so no line number in the render region moved. Only the
tail anchor differs (pi's `+360`-line SHELLUP block sits between the body and the file tail).

| # | v1 (`33dc7811`) | v2 (`8131cd2d`) | old→new | N→N |
|---|---|---|---|---|
| 1 | `@@ -5270,7 +5270,7 @@` | `@@ -5270,7 +5270,7 @@` | 7→7 | yes |
| 2 | `@@ -5357,20 +5357,20 @@` | `@@ -5357,20 +5357,20 @@` | 20→20 | yes |
| 3 | `@@ -5392,7 +5392,7 @@` | `@@ -5392,7 +5392,7 @@` | 7→7 | yes |
| 4 | `@@ -5461,12 +5461,12 @@` | `@@ -5461,12 +5461,12 @@` | 12→12 | yes |
| 5 | `@@ -5494,7 +5494,7 @@` | `@@ -5494,7 +5494,7 @@` | 7→7 | yes |
| 6 | `@@ -5511,7 +5511,7 @@` | `@@ -5511,7 +5511,7 @@` | 7→7 | yes |
| 7 | `@@ -5522,14 +5522,14 @@` | `@@ -5522,14 +5522,14 @@` | 14→14 | yes |
| 8 | `@@ -5634,7 +5634,7 @@` | `@@ -5634,7 +5634,7 @@` | 7→7 | yes |
| 9 | `@@ -5649,7 +5649,7 @@` | `@@ -5649,7 +5649,7 @@` | 7→7 | yes |
| 10 | `@@ -5660,35 +5660,35 @@` | `@@ -5660,35 +5660,35 @@` | 35→35 | yes |
| 11 (tail) | `@@ -8624,3 +8624,167 @@` | `@@ -8984,3 +8984,167 @@` | 3→167 | growth, at the tail |

`git diff --stat`: `1 file changed, 220 insertions(+), 56 deletions(-)` — byte-for-byte the same stat
line v1 carried. File length `8986 → 9150` lines (+164, all at the tail).

**GATE-FAMILY (condition c).** `grep -c 'fn render_service\|fn x86_render_service\|fn orin_render_service' main.rs`
= **3 before, 3 after**; `grep -c 'fn render_pass' main.rs` = 0 → 1. Step 1 does not change the family
size, and the S7 row keeps its `open` status and its expiry clause.

## 4. Gates

All run in the worktree at `8131cd2d` with `S7-STEP1-v2.patch` applied (the baseline legs on the same
tree with it reverted). `unaos/target/user_blob.bin` was seeded from the orin worktree — a fresh
worktree has none and it type-checks identically (`arroyo` says so at `:3353`).

| # | gate | command | result |
|---|---|---|---|
| 1 | type-check both arches | `cd unaos && ./arroyo check` | **exit 0**; **65 ✅ / 0 ❌** leg lines; `✅ kernel cfg coverage OK (45 legs)`; `✅ knob→leg coverage OK`; `GATE-KNOB: OK — 154 features declared, 153 named by a cfg, 0 phantom, 0 dead`; `GATE-LEDGER: OK — 75 rows` |
| 2 | witness count | §2's awk | **4 = 4** |
| 3 | Pi regression, bench geometry | `UNAOS_FBW=1920 UNAOS_FBH=1200 ./arroyo kernel8-test 210` | **exit 0**; `✅ MBENCH PASS — 120/120 required witnesses, 0 forbidden hit(s), 26990 lines scanned` (M = `pi4-regression.spec`'s 118 REQUIRE + 2 COUNT); banner `⚡ kernel features: baremetal,skip_xhci,witness` |
| 4 | x86 with the compositor | `UNAOS_WC=1 ./arroyo build`, unpatched then patched | **exit 0** both; banner `⚡ kernel features: ehcihid,kbdwit,sdhcblk,smolnet,wc` both; **byte-identical** (§5) |
| 5a | v1 at pi's base | `git apply --check S7-STEP1.patch` | **exit 1** — `error: patch failed: unaos/crates/kernel/src/main.rs:5357` / `patch does not apply` |
| 5b | v2 at pi's base | `git apply --check S7-STEP1-v2.patch` (and `--cached` against a scratch index read from `8131cd2d`) | **exit 0** |
| 5c | tail fragment at jetson's tip | `GIT_INDEX_FILE=… git read-tree a05c2c8e && git apply --check --cached S7-STEP1-v2-tail.patch` | **exit 0** |

### 4.1 The witnesses are alive, not merely present

The grep in §2 proves the four statements are still in the file. The `kernel8-test` capture proves the
two that MOVED still fire, on the same cadence, from their new home (`unaos/target/serial-pi.log`,
`witness` on):

```
[shellup] census t=12895ms desktop=UNARMED render=live passes=+48 shell=none:unminted win=0
          pulse=0 quarry=closed gui=sent48/recv48 depth=0 app_active=false == witness ::
[u7stk] at=render:pass task=71:render sp=0x25d9590 low=0x25d1ba0 top=0x25d9ba0 len=32768
        used=1552 hw=2952 headroom=29816                                        (×9 in the run)
```

`render=live passes=+48` IS `SHELLUP_RENDER_HB`, read from the input core — the counter now bumped
inside `ChannelWait::wait`. `at=render:pass` IS the `stk_probe` fold, now inside `ChannelWait::census`.
`grep -c 'REFUSING corrupt switch-in'` on the capture is **0** (the SPIN-6 FORBID the pi spec carries).
`[sched6] passes=…/s composites=…/s mean=… cyc/pass (dirty-paced strip@250ms)` is on the wire unchanged
in shape, 40 lines in the run — the census still prints from `ChannelWait::census`.

### 4.2 The Pi knob-off image — it MOVES, as the design's outcome (b)

| image | before (`8131cd2d`) | after (v2) | size |
|---|---|---|---|
| `target/pi_baremetal/kernel8.img`, `./arroyo kernel8` (knob-off, `K8_FEATS=baremetal,skip_xhci`) | `77690c779c3a6a9589f951f2625359edc453a4c8daff4ce4ddc792cb7090ea83` | `b5c0a3a19485225ce8d6595b33dd0f2caee9c1e0aedbfbe1539864259e010aa9` | 1,262,416 B both |

**This is not identity and is not claimed as one.** The converted body IS the Pi's, so the Pi image is
the one that has to move; the cause is the same one v1 stated in advance and measured
([`../orin14/S7-STEP1.md`](../orin14/S7-STEP1.md) §3.3): the monomorphised
`render_pass::<ChannelWait>` sorts ahead of the root CGU's DefIndex-ordered items, which permutes the
function order and shifts every relocation behind it. The patched build was run twice and produced
`b5c0a3a1…` both times, so the measurement is deterministic.

Re-measured on pi's body (`cmp -l | wc -l` = 913,613 of 1,262,416):

| section | differing / size | class |
|---|---|---|
| `.text.boot` | 2 / 76 | one immediate into shifted `.text` |
| `.text` | 770,165 / 1,006,656 | the function-order permutation + the body's re-codegen + the shim |
| `.rodata` | 59,583 / 221,348 (compared at its 16-byte shift) | the same permutation seen from data |
| `.data` | **0** / 32,336 | — |

Per-function, modulo relocation ([`../orin14/S7-STEP1-fncmp.py`](../orin14/S7-STEP1-fncmp.py), re-run
on this tree's two ELFs):

```
$ python3 S7-STEP1-fncmp.py kernel8-before.elf kernel8-after.elf
functions: before=1360 after=1362 common=1360 identical(mod-reloc)=1358 differing=2
only-before: []
only-after: ['_RINvCs…_12unaos_kernel11render_passNtB2_11ChannelWaitEB2_', '__CortexA53843419_129004']
DIFF _RNvCs…_12unaos_kernel14render_service: 603 -> 2 insns
DIFF _RNvNtCs…_12unaos_kernel3pal13pump_and_poll: 195 -> 195 insns
```

**1358 of 1360 functions are instruction-for-instruction identical modulo relocation.** The two that
are not:

1. `render_service` 603 → 2 instructions — it became the shim. That is the conversion.
2. `pal::pump_and_poll`, 195 → 195 instructions, differing in **exactly one**:

   ```
    adrp x8, <REL>
    ldr x20, [x8, #PGOFF]
   -str xzr, [x8, #PGOFF]
   +b <__CortexA53843419_129004>
    nop
   ```

   This is **LLD's Cortex-A53 erratum-843419 workaround**, not a codegen change. The permutation moved
   that `adrp` to `0x128ffc` — the last instruction of a 4 KiB page — which is the erratum's trigger
   pattern, so the linker displaced the following `str` into a thunk:
   `__CortexA53843419_129004: str xzr, [x8, #0x978]; b 0x129008`. The displaced instruction is carried
   verbatim and control returns to the next address. It is a pure ADDRESS artifact of the same
   permutation, which is why `.text` grew by exactly 8 bytes (`0xf5c40 → 0xf5c48`) and `.rodata` shifted
   by 0x10; `.data` and `.bss` are unmoved and the flat image is the same length.

   v1 did not see this thunk because it was linking a different tree at different addresses. It is
   worth naming rather than folding into "the permutation", because a reader diffing the two
   disassemblies will hit it and it looks like a code change until you read the thunk.

## 5. x86 identity (rmbp 12 condition a)

The line-neutrality proof. Every x86 panic `Location` below the Pi region would move if any line in
`main.rs`'s render region had shifted; the ten mid-file hunks are N→N and the growth is at the tail,
so nothing shifts and the x86 kernel must be byte-identical. Two builds on the same tree,
`main.rs` the only difference:

```
$ cd unaos && UNAOS_WC=1 ./arroyo build          # unpatched, then patched — exit 0 both
$ sha256sum x86-before.elf x86-after.elf
a2cdfd3733f42169600f01a009e582cbab3a5b86af72878cae1d2380cd1e96a4  x86-before.elf
a2cdfd3733f42169600f01a009e582cbab3a5b86af72878cae1d2380cd1e96a4  x86-after.elf
$ cmp x86-before.elf x86-after.elf ; echo $?
0
$ llvm-objcopy -O binary x86-{before,after}.elf x86-{before,after}.bin
$ sha256sum x86-before.bin x86-after.bin
9066c547d875197dfd904b6935db9ce87c1188f34770817faa5a149bce09b783  x86-before.bin
9066c547d875197dfd904b6935db9ce87c1188f34770817faa5a149bce09b783  x86-after.bin
```

| artifact | before | after | verdict |
|---|---|---|---|
| `target/x86_64-unaos/release/unaos-kernel` (ELF, 2,066,896 B) | `a2cdfd37…` | `a2cdfd37…` | **IDENTICAL** (`cmp` exit 0) |
| `objcopy -O binary` of it | `9066c547…` | `9066c547…` | **IDENTICAL** |

Both builds carry `⚡ kernel features: ehcihid,kbdwit,sdhcblk,smolnet,wc` — `wc` armed, per CLAUDE.md.
This is a BUILD-identity proof, not a behavioural compositor gate: the change compiles to nothing on
x86 (the whole block is `cfg(all(aarch64, baremetal))`), so the claim being tested is line-neutrality,
and identity is the only outcome that proves it.

## 6. What pi 7 acks, what rmbp 12 reviews

**pi 7 — the ack.** The four folded witnesses survive the conversion (§2's count, §4.1's wire), the
pi-only livecon comment text is preserved verbatim, the pi regression suite is **120/120, 0 forbidden**
at bench geometry, and the knob-off `kernel8.img` **moves** — `77690c77…` → `b5c0a3a1…` — for the
cause stated in advance and measured in §4.2 (1358/1360 functions identical modulo relocation; `.data`
untouched). That is design §4's ranked outcome **(b)**, and it needs the same baseline re-base pi did
for CAPREVOKE. What is being asked of pi 7 is a decision **in pi's own session**, on pi's own tree:
(i) accept the moved knob-off baseline for S7 step 1, and (ii) confirm the two relocated witnesses
belong where §1 put them — on their host statements inside `ChannelWait` — rather than in the body.
One item is flagged and NOT changed: `main.rs:5373`'s comment names `` `video/pidesk.rs` ``, a file
this tree does not have (it is `video/desktop_firmware.rs`); that is pi's line to fix or keep.

**rmbp 12 — the five conditions.**

| condition (batons/orin-15.md §Round 2) | where it is answered |
|---|---|
| (a) x86 `UNAOS_WC=1` byte identity by a two-build diff | §5 — ELF and flat binary both identical, `cmp` exit 0, `wc` in both banners |
| (b) pi's ack in THEIR session | not this document's to give; §6 states exactly what is being asked |
| (c) GATE-FAMILY 3 + S7 keeps its expiry | §3 — `grep -c` = 3 before and after, `fn render_pass` 0→1; the LEDGER S7 status stays `open` and its expiry clause is untouched |
| (d) N→N hunks, growth at the tail | §3 — ten mid-file hunks N→N at v1's exact line numbers, one tail hunk 3→167 |
| (e) [`../orin14/S7-STEP1.md`](../orin14/S7-STEP1.md) §3 read against the patch | §4.2 — §3's mechanism re-measured on pi's body; §3.3's "1350/1351 identical" becomes 1358/1360 here, and the one NEW difference (`pump_and_poll`) is identified as LLD's A53-843419 thunk, an address artifact, with the thunk's own disassembly quoted |

**The grant that is still not in hand.** `main.rs` is rmbp's shared-kernel-core lane. Nothing here is
merged; the deliverables are two patch files and this document. The code lands when rmbp grants the
file and pi acks the baseline — in that order or either, but both before a merge.
