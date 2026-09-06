# GA10B-RUNG3 — rungs 3 and 3b as built (`ga10bprobe3` / `ga10bprobe3b`, `UNAOS_GA10B_PROBE3=1|2`)

Seat orin 16, executor GA10B3, 2026-09-06. Spec of record for the code in
`unaos/crates/kernel/src/arch/aarch64/ga10b_probe.rs` (tail, after the rung-2 block). The design this
implements is [`../orin14/GA10B-LADDER.md`](../orin14/GA10B-LADDER.md) §Rung 3 and §Rung 3b; the
predecessor's flight is [`PROBES-2026-09-06.md`](PROBES-2026-09-06.md) §4 and
[`ga10bprobe2-boot1.log`](ga10bprobe2-boot1.log); the register provenance is the ACKED facts file
[`ga10b-probe-rung1.facts.md`](../../../../unaos/docs/dev/OS/09_PLATFORM/ga10b-facts/ga10b-probe-rung1.facts.md)
(§6, ACK-WITH-EDITS 2026-08-25). Ledger row: `A28` in
[`../../OS/orin-ledger.md`](../../OS/orin-ledger.md).

**Clean room.** This executor read no `nvgpu`. Every offset and bit below is either (a) a pointer-backed
entry of the ACKED facts file, (b) the live DTB, (c) a permissively-licensed public header (Linux
`bpmp-abi.h`, SPDX `GPL-2.0 OR MIT`), or (d) explicitly labelled **PUBLIC-RECALLED** — there is exactly
one such item, the Falcon `MAILBOX0` pointer, and it is a WRITE target, so its recalled status is
printed on the wire beside the write.

---

## 0. What changed, and the two knobs

| knob | env | features | what runs |
|---|---|---|---|
| rung 3 | `UNAOS_GA10B_PROBE3=1` | `ga10bprobe3` (implies `tegra`) | the read-only register pass, inside rung 2's bracket |
| rung 3 + 3b | `UNAOS_GA10B_PROBE3=2` | `+ ga10bprobe3b` (implies `ga10bprobe3`) | the same, then the ladder's FIRST GA10B MMIO writes |

ONE env knob with two values, deliberately: rung 3b is a write rung, so the read-only rung must be
flyable alone if the bench wants that first. Any other non-empty value arms rung 3 alone — an
unexpected value never buys a write.

`ga10bprobe3` is a **third sibling** of `ga10bprobe1`/`ga10bprobe2`, never their dependent. Like rung
2, `ga10bprobe3_run` **RETURNS**, so the flight is a full boot and the desktop comes up behind it;
its call site sits BETWEEN rung 2's and rung 1's in `tegra_early_stop`'s BPMP block, because rung 1's
`-> !` PSCI `SYSTEM_OFF` must never run in front of a rung that has to reach the desktop. The flight
therefore arms rung 3 **without** `UNAOS_GA10B_PROBE1`.

Matrix legs (`arroyo` `KERNEL_CFG_MATRIX`): `arm-tegra-ga10bprobe3` (rung 3 alone — the polarity in
which every `ga10bprobe3b` item is cfg-erased, which is the dead-code trap a 3b-only leg would hide)
and `arm-tegra-ga10bprobe3b` (the only leg that type-checks rung 3b's armed polarity). Both green.

**Byte identity.** Every edit to already-compiled code is appended to an existing line (`main.rs`
call site, `arch/aarch64/mod.rs` module gate), so no panic `Location` moves knob-off. Measured:
`target/pi_baremetal/kernel8.img` sha256
`8ff7c1d1f4e8938d9a29df4a094ecc1fe01684350adeef8a577b13c5eb89dc13` (1,254,984 B) before and after
this arc — identical.

---

## 1. Rung 3's FIRST question — DTB clock id 236 and BPMP's `err=-22`

Rung 2 read the `gpu@` node's `clocks` list as **304, 41, 236** and got `err=0` on `MRQ_CLK
CMD_CLK_IS_ENABLED` for 304 and 41 but **`err=-22` for 236**, before AND after
([`ga10bprobe2-boot1.log`](ga10bprobe2-boot1.log)).

**Which clock 236 is.** The list is positional and the DTB is the authority: entry 0 = `304` =
`TEGRA234_CLK_GPUSYS` (`clock-names` "sysclk"), entry 1 = `41` = `TEGRA234_CLK_GPC0CLK` ("gpc0clk"),
entry 2 = `236` = **`TEGRA234_CLK_GPC1CLK`** ("gpc1clk") — the second GPC's clock on a die whose
`top_num_gpcs` rung 1 measured as 2. The numbers are Linux
`include/dt-bindings/clock/tegra234-clock.h`; the ORDER is the live DTB's, printed by the rung-2
flight. Nothing here is a guess about our DTB read: the DTB is what named 236.

**Why BPMP refuses it — the hypothesis, and the measurement that settles it.** `-22` is
`-BPMP_EINVAL` from `bpmp-abi.h`'s error table: an **argument** rejection, not an `-EACCES`-class
policy refusal (rung 2 saw no `-EACCES` anywhere). The standing hypothesis is that this BPMP
firmware's clock table carries no queryable entry for that id on this SKU, so the subcommand is
refused on its argument. That is a hypothesis, not a measurement, so rung 3 measures it with three
**pure queries** and states a per-clock verdict — 236 read against two same-boot controls (304, 41),
never alone:

| query | request word | what it decides |
|---|---|---|
| `CMD_CLK_GET_MAX_CLK_ID` (15) | `15 << 24` | is 236 above this firmware's highest id at all? |
| `CMD_CLK_GET_ALL_INFO` (14) | `(14 << 24) \| id` | does BPMP know this clock (words 0/1 = `flags`, `parent`)? |
| `CMD_CLK_GET_RATE` (1) | `(1 << 24) \| id` | a second, independent per-clock query (words 0/1 = rate lo/hi) |

Verdict arms, mutually exclusive, one line per clock
(`[ga10bprobe3] clk <id> identity: is_enabled_err=… info_err=… in_range=… -> …`):

* `BPMP-MANAGED` — `IS_ENABLED` answered `err=0` (expected for 304 and 41).
* `NOT-IN-BPMP-TABLE (out of range)` — id > `GET_MAX_CLK_ID` and `GET_ALL_INFO` refused it too.
* `NOT-IN-BPMP-TABLE (in range, no entry)` — `GET_ALL_INFO` refused it, id within range.
* `IN-TABLE-BUT-ENABLE-STATE-REFUSED` — `GET_ALL_INFO` answered `err=0`: BPMP knows the clock and
  only the enable-state subcommands are refused for it (a per-clock capability, not a missing id,
  and not the wrong MRQ).

The three hypotheses the ladder named ("wrong id, not a BPMP-managed clock, or a different MRQ") map
onto those arms exactly, and the third is excluded by construction — `MRQ_CLK` is the MRQ that
answered for 304 and 41 on the same channel in the same boot.

`GET_ALL_INFO`'s response also carries a name string and a parent list past the two payload words
`Chan::transfer` returns. Rung 3 deliberately does **not** widen that shared transport: a probe rung
does not get to edit the channel every other Tegra subsystem uses. (The response is 113 bytes and the
IVC frame's payload region is 120, so nothing overflows either.)

---

## 2. Rung 3 — the bracket

Rung 2's proven bracket, reused verbatim in shape and sharing its helpers (`pg_state`, `clk`,
`settle_ms`) so the two rungs cannot drift:

1. DTB RAM walk (no MMIO) — `gpu@` `reg[0]` → BAR0, `power-domains` odd word → domain id, `clocks`
   odd words → up to 8 BPMP clock ids, printed as found.
2. `MRQ_PG GET_STATE id` → `pg-before`. Timeout → `REFUSED reason=pg-timeout`, RETURN.
3. Only if the pre-state ≠ ON: `MRQ_PG SET_STATE id ON` (announced), 2 ms settle.
4. `MRQ_PG GET_STATE id` again → `pg-readback` — **the explicit ON that alone earns a BAR0 touch.**
5. Per clock: `IS_ENABLED` (before) → `ENABLE` if off → `IS_ENABLED` (after); census line.
6. The clock-identity block of §1 (pure queries), 2 ms settle.
7. The register pass of §3, only if the readback said ON; else
   `REFUSED reason=pg-on-refused|pg-readback-not-on` and **no** register is read and **no** write is
   attempted.
8. (`=2` only) rung 3b, §4 — after the read list is exhausted, so a fatal WRITE can never be
   confused with a fatal READ.
9. Symmetric restore: `DISABLE` every clock this rung enabled (reverse order); `SET_STATE OFF` +
   `GET_STATE` only if this rung powered the domain on. `restored:` line, then
   `rung 3 complete — RETURNING`.

**Scope fence.** No display, no fabric, no vendor pad block. Every address is inside the `gpu@` BAR0
aperture the DTB declares. The FWALL/nvdisplay SError convictions
([`../orin14/GA10B-HISTORY.md`](../orin14/GA10B-HISTORY.md) S2/S4/S5 — the boot7e window sweep, ESR
`0xbe000011`) are untouched by this rung.

---

## 3. Rung 3 — the register table, as built

25 registers, in the ladder's risk order. Every one is announced before it is touched
(`about-to-read <name> reg=0x<abs> (class=…) — if this is the LAST line, THAT read was EL3-fatal`),
and each **address class** is announced before its first touch with an honest NEW/KNOWN label. Two
classes are NEW this boot: **mc** and **pmu-falcon2**, and the PMU goes LAST by design.

Result line shape: `[ga10bprobe3] <name> @0x<BAR0-relative off> = 0x<v> expect=…`, or
`= -UNREADABLE reason=all-ones|pri-error …`. `<off>` is BAR0-relative; the absolute address is in the
announce line above it. On this board BAR0 = `0x17000000`, so absolute = `0x17000000 + off`.

| # | class | name | off (BAR0-rel) | expected | provenance |
|---|---|---|---|---|---|
| 1 | fuse (KNOWN) | `fuse_opt_sec_debug_en` | `0x821040` | datum | facts (b) Security-state fuses |
| 2 | fuse | `fuse_opt_wpr_enabled` | `0x8205ec` | datum — **rung-4 input** | facts (b) |
| 3 | fuse | `fuse_opt_vpr_enabled` | `0x82067c` | datum | facts (b) |
| 4 | **mc (NEW)** | `mc_enable` | `0x000200` | datum: which engines UEFI left enabled | facts (b) Die-characterization |
| 5 | mc | `mc_elpg_enable` | `0x00020c` | xbar `0x4` / l2 `0x8` / hub `0x20000000` bits, decoded on their own line | facts (b) |
| 6 | top (KNOWN) | `top_device_info_cfg` | `0x0224fc` | `version_init = 0x2`; the `device_info2` walk is rung 5's | facts (b) |
| 7 | gsp-falcon-v1 (KNOWN) | `gsp_falcon_hwcfg` | `0x110108` | datum: IMEM/DMEM sizes (rung 4's load input) | facts (b) Legacy Falcon regs |
| 8 | gsp-falcon-v1 | `gsp_falcon_dmactl` | `0x11010c` | `require_ctx` bit0, decoded | facts (b) |
| 9 | gsp-falcon-v1 | `gsp_falcon_idlestate` | `0x11004c` | datum | facts (b) |
| 10 | gsp-falcon-v1 | `gsp_falcon_irqmask` | `0x110018` | datum | facts (b) |
| 11 | gsp-falcon-v1 | `gsp_falcon_irqdest` | `0x11001c` | datum | facts (b) |
| 12 | gsp-falcon-v1 | `gsp_falcon_cpuctl_v1` | `0x110100` | `halt_intr` bit4, decoded (rung 1 read the priscv view as `0x10`) | facts (b) |
| 13 | gsp-priscv-bcr (KNOWN) | `priscv_bcr_ctrl` | `0x111668` | **0** — no BCR programmed (rung 1: `br_retcode=0`) | facts (b) RISC-V boot-ROM interface |
| 14 | gsp-priscv-bcr | `priscv_bcr_dmacfg` | `0x11166c` | `lock_locked` bit31 — **rung-4 input** | facts (b) |
| 15 | gsp-priscv-bcr | `priscv_bcr_pkcparam_lo` | `0x111670` | 0 | facts (b) |
| 16 | gsp-priscv-bcr | `priscv_bcr_pkcparam_hi` | `0x111674` | 0 | facts (b) |
| 17 | gsp-priscv-bcr | `priscv_bcr_fmccode_lo` | `0x111678` | 0 | facts (b) |
| 18 | gsp-priscv-bcr | `priscv_bcr_fmccode_hi` | `0x11167c` | 0 | facts (b) |
| 19 | gsp-priscv-bcr | `priscv_bcr_fmcdata_lo` | `0x111680` | 0 | facts (b) |
| 20 | gsp-priscv-bcr | `priscv_bcr_fmcdata_hi` | `0x111684` | 0 | facts (b) |
| 21 | gsp-priscv-bcr | `priscv_boot_vector_lo` | `0x111380` | datum | facts (b) |
| 22 | gsp-priscv-bcr | `priscv_boot_vector_hi` | `0x111384` | datum | facts (b) |
| 23 | gsp-priscv-bcr | `priscv_riscv_irqmask` | `0x111528` | datum | facts (b) |
| 24 | gsp-priscv-bcr | `priscv_riscv_irqdest` | `0x11152c` | datum | facts (b) |
| 25 | **pmu-falcon2 (NEW)** | `pmu_falcon2_cpuctl` | `0x10b388` | `halted` bit4, decoded — a SECOND engine | facts Aperture framing (PMU falcon2 base `0x10b000`) + (b) priscv `cpuctl` `0x388` |

Then, unconditionally after the pass, the **two rung-4 inputs on their own summary lines**:

```
[ga10bprobe3] bcr_dmacfg lock_locked=<0|1> (raw=0x…) — …
[ga10bprobe3] opt_wpr_enabled=0x…
[ga10bprobe3] pg=0x1 clk=<n>/<t> regs=<r> of 25 readable, <u> UNREADABLE -> COMPLETE
```

Either summary reads `-UNREADABLE` if its register did not answer — the rung never prints a bit
extracted from an all-ones or `0xBADxxxxx` value.

**Priv-lockdown expectation.** Rung 1 measured `falcon_hwcfg2` bit13 = 1 (BR priv-lockdown engaged),
so the priscv BCR reads (#13–24) are the ones most likely to come back `-UNREADABLE`. That is the
datum, reported per register; it is not a failure of the rung.

---

## 4. Rung 3b — the first GA10B MMIO writes

Runs only under `ga10bprobe3b`, only inside rung 3's bracket, only after rung 3's read list is
exhausted. **At most three writes, each announced on its own line before it happens.**

| step | register | abs (BAR0 `0x17000000`) | value | provenance |
|---|---|---|---|---|
| 1 WRITE | `pgsp_falcon_engine` | `0x171103c0` | `0x1` (reset ASSERT, bit0) | facts (b) GSP engine reset, marked `[WRITE — probe omits]` for rung 1 |
| — | hold | — | **1 ms** (facts require ≥ 10 µs; 1 ms is ~100× margin, never a shorter hold) | facts (b) |
| 2 WRITE | `pgsp_falcon_engine` | `0x171103c0` | `0x0` (DEASSERT) | facts (b) |
| 3 READ | `priscv_cpuctl` | `0x17111388` | `halted` bit4 expected 1 | facts (b) |
| 4 WRITE | `gsp_falcon_mailbox0` | `0x17110040` | `0x5a5aa5a5` | **PUBLIC-RECALLED** (nouveau `nvkm/falcon`; open-gpu-kernel-modules `dev_falcon_v4.h`; both MIT) — NOT in the ACKED facts file; corroborated only by that file's matching v1 offsets. Printed as recalled on the wire. |
| 5 READ | `gsp_falcon_mailbox0` | `0x17110040` | the readback | same |

**The gate on step 4.** The mailbox write happens ONLY if step 3's read is sane — not `0xFFFFFFFF`,
not the PRI `0xBADxxxxx` pattern. Otherwise:
`[ga10bprobe3b] -> MAILBOX-SKIPPED reason=cpuctl-all-ones|cpuctl-pri-error`, and rung 3b ends.

**Witness:**

```
[ga10bprobe3b] mailbox0 wrote=0x5a5aa5a5 read=0x…
[ga10bprobe3b] -> MAILBOX-HELD | MAILBOX-MISMATCH read=0x…
[ga10bprobe3b] rung 3b complete
```

**Restore.** None is possible for a reset and none is needed: the engine was halted and never-booted
before (rung 1: `br_retcode=0`, `cpuctl=0x10`) and is halted and never-booted after. Rung 3's
`restored:` line says so explicitly.

**The write helper.** `w32` is the module's ONLY `write_volatile` and is itself
`#[cfg(feature = "ga10bprobe3b")]`, so rung 1, rung 2 and rung 3-alone still compile with no write
path to a GA10B register at all. The rung-1 header's absolute "there is no `write_volatile` in this
module" wording is corrected in place rather than left standing as a false invariant.

---

## 5. PASS / FAIL / UNREADABLE, and the STOP rule

**Rung 3 PASS** = the read list is exhausted — last rung-3 line `rung 3 complete — RETURNING`,
preceded by `-> COMPLETE`, with `bcr_dmacfg lock_locked=<0|1>` and `opt_wpr_enabled=0x…` both
recorded (both are rung 4's inputs) — and the boot continues (a `[deskcascade] -> CASCADED` line
after it).

**Per-register UNREADABLE** is a first-class datum, not a failure: `-UNREADABLE reason=all-ones` or
`reason=pri-error`. A rung 3 in which every priscv BCR register is UNREADABLE is a **PASS with a
finding** (priv-lockdown covers the BCR block), and it is the answer rung 4 needs.

**Rung 3 REFUSED** = `-> REFUSED reason=<no-gpu-node|no-power-domains|pg-timeout|pg-on-refused|
pg-readback-not-on>` — no register was read, no write attempted; the reason is the datum.

**Rung 3b PASS** = `mailbox0 wrote=0x5a5aa5a5 read=0x5a5aa5a5` → `-> MAILBOX-HELD`.
**Rung 3b measured-negative** = `-> MAILBOX-MISMATCH read=0x…` (the recalled pointer is the first
thing to re-verify) or `-> MAILBOX-SKIPPED reason=…` (the reset did not leave the engine readable).
Both are results, not failures.

**FAIL-fatal (the STOP rule).** If the LAST line on the wire is an `about-to-read …` or an
`about-to-WRITE …` announce, THAT access was fatal in this state. Record the exact line, **stop** —
do not re-fly with the step removed on the bench's own initiative — and report: which register,
which class, and whether it was a read or a write. Per R19 the path stays open and recorded as
"failed under these conditions", never "ruled out": JX1's XUSB read was EL3-fatal until JB1c turned
the rail on, and the address was innocent all along.

---

## 6. Provenance delta over §Rung 3 of the ladder

Everything the ladder's §Rung 3 table named is implemented at the offset it gave. Three things are
new in this build and are recorded here because the ladder did not carry them:

1. **The clock-identity block** (§1) — `CMD_CLK_GET_RATE` 1, `CMD_CLK_GET_ALL_INFO` 14,
   `CMD_CLK_GET_MAX_CLK_ID` 15, and `BPMP_EINVAL` 22, all from Linux
   `include/soc/tegra/bpmp-abi.h` (SPDX `GPL-2.0 OR MIT`) — the same header `bpmp_tegra.rs` already
   cites. All three are pure queries; none mutates anything.
2. **The address-class NEW/KNOWN labels** — derived from rung 1's flight readback (which classes
   answered without fault) and stated on the wire rather than assumed.
3. **The 1 ms reset hold** — the ladder says ≥ 10 µs; this module's coarsest bounded wait is
   `settle_ms`, so the hold is 1 ms. Longer than required is always safe for an assert-to-deassert
   minimum; shorter would not be.

Still UNKNOWN and untouched by this rung, exactly as the ladder says: the L4T blob filenames and the
licence question (rung 4, Peter's), the GSP RPC/message-queue layout, the ACR WPR descriptor, and
everything in rungs 5–6 that needs a second Group A facts pass.
