# GA10B-LADDER — the boot-by-boot ladder from rung 1 to a 3D pipe

**Ruling.** Peter, 2026-09-06 (`docs/dev/RULINGS.md` R18): "i thought we can probe the hardware boot
by boot to make the GA10B work". The `orin-ledger` §F row said RULED OUT because the GA10B's microcode
is signed and encrypted with boot-ROM-enforced verification; that hardware fact stands and is rung 4's
wall. The ruling is that the ladder gets climbed anyway, one rung per boot, each rung answering exactly
one question with a PASS/FAIL predicate, so that every boot buys a fact and the wall — if it is one —
is measured, not assumed.

**Discipline (inherited from rung 1, `ga10b_probe.rs` header; JX1/JX2/JX3):** BPMP power gate proven
first; announce-before-touch for every new MMIO address class (the last line names the killer);
one new address class or one BPMP transaction per boot; witness tokens > 8 bytes; every mutation
symmetric where the rung's question allows it. The vendor-pad SError conviction in `sdmmc_tegra`
(orin-ledger §"five gaps" 3, the boot-4e EL3 window) is the precedent for "a write that kills the
boot": every write below is named as a write and lands on its own rung.

**Clean room (`docs/MANIFESTO/CLEAN_ROOM_POLICY.md` §6, 2026-08-25 disposition; orin-ledger D3).**
This ladder's author read no `nvgpu` — the §6 group boundary makes whoever extracts facts Group A,
and Group A may not write the implementation; rung 2's code is in this same commit, so this seat stays
Group B. Everything below is sourced from (a) the ACKED rung-1 facts file
[`ga10b-probe-rung1.facts.md`](../../../../unaos/docs/dev/OS/09_PLATFORM/ga10b-facts/ga10b-probe-rung1.facts.md),
(b) the live DTB (EXT), (c) rung 1's flight readback, (d) public permissively-licensed sources
(NVIDIA `open-gpu-kernel-modules` published headers and `nv_arch.h` — MIT; `nouveau` nvkm — MIT;
`envytools` rnndb; Linux `bpmp-abi.h` — GPL-2.0 OR MIT; Linux dt-bindings — the same headers the
tree already cites for XUSB/PWM ids). Facts the ladder NEEDS and none of those carry are marked
**UNKNOWN — Group A pass**: a future quarantined extraction (facts file + independent terms review,
§6) by a seat that will not write the code. **NVIDIA's signed firmware blobs** (the L4T
`/lib/firmware/nvidia/ga10b/` set — rung 4) are named, never fetched, never embedded: the licence
question is Peter's (`orin-3d.md` §3, "the blob law").

---

## The ladder at a glance

| rung | boot | question | writes | new address class | status |
|---|---|---|---|---|---|
| 1 | o3d (flown 2026-08-25) | Is the rail on, has the boot ROM ever run, is the block priv-locked, is the die GA10B? | none | BAR0 fuse / GSP falcon / GSP priscv / top | **FLOWN — all expected** (below) |
| 2 | next | With the domain ON and the DTB clocks ENABLED (via BPMP), does PMC_BOOT_0 answer with an Ampere chip id — and which clocks did UEFI leave running? | BPMP MRQs only (PG SET_STATE, CLK ENABLE/DISABLE), symmetric | BAR0 + 0x0 (PMC) | **fixed-unflown — this commit** (`ga10bprobe2`) |
| 3 | +1 | What did the platform firmware leave in the GSP boot-config (BCR) registers, the PMU falcon, the security fuses and MC enables? Is any secure image staged? | none | GSP priscv BCR block, PMU falcon2, fuse (3 more), MC | designed (below) |
| 3b | +1 | Does the GSP falcon come out of an engine reset halted, and does a Falcon MAILBOX scratch write read back? | **first GA10B MMIO writes**: pgsp_falcon_engine reset assert/deassert; one MAILBOX write | none new (GSP falcon) | designed; the first write rung |
| 4 | licence gate | Can the RISC-V boot ROM be handed a signed FMC image through the BCR DMA path and report PASS? | BCR DMA addr/cfg, cpuctl startcpu | none new | **blocked on Peter's blob ruling**; protocol in facts, blobs named |
| 5 | +N | Can the host FIFO run one copy-engine job (a DMA copy we can read back)? | PBDMA/runlist/channel setup (many) | BAR0 host (PFIFO/PBDMA), CE PRI | UNKNOWN facts — Group A pass |
| 6 | +M | 3D: one triangle through the GR pipe, checked against the `rast` oracle | GR/FECS/GPCCS (needs rung 4) | GR PRI, GPC/TPC | UNKNOWN facts — Group A pass; behind rung 4 |

---

## Rung 1 — what the o3d flight established (the floor this ladder stands on)

Capture `~/unaos-bench/capture/line-acm0/orin.log` (MARK o3d, `3ffa56c`, knobs=GA10B_PROBE1), 16
`[ga10bprobe1]` lines, every one of the design note's "powered-on, never-booted GPU" expectations met:

| datum | value | meaning |
|---|---|---|
| `gpu@` BAR0 (DTB reg[0]) | `0x17000000` | the aperture (16 MiB `gpu@17000000`) |
| power-domain id (DTB) | `35` | = `TEGRA234_POWER_DOMAIN_GPU` (dt-bindings) |
| MRQ_PG GET_STATE 35 | `err=0 state=0x1` | **the rail was ALREADY ON at the raw handoff** — UEFI leaves the GPU partition powered |
| fuse `opt_priv_sec_en` 0x17820434 | `0x1` | production secure boot fused (the rung-4 wall, measured) |
| GSP `falcon_hwcfg2` 0x171100f4 bit13 | `0x0001b733`, bit13=1 | BR priv-lockdown engaged |
| GSP priscv `br_retcode` 0x1711165c | `0x0` | boot ROM never reached a verdict — no GPU firmware was booted by MB2 |
| GSP priscv `cpuctl` 0x17111388 | `0x10` (bit4) | RISC-V core halted |
| `top_num_gpcs` 0x17022430 | `2` | GA10B Orin Nano die identity |

Consequences for rung 2: (1) "power-on" is expected to be a **no-op** on this board — the rung's
`SET_STATE ON` is issued only if the pre-state is not ON, and the pre-state is on the wire either way;
(2) five PRI reads across four blocks answered sanely under UEFI's clock state, which is the only
empirical bound we have on "a BAR0 read with the GPU clocks in an unknown state"; (3) an unpowered
BAR0 read has **never been observed** on GA10B — rung 1 refuses before it, and JX1's precedent (a
gated Tegra block read → SError ESR 0xbe000011, EL3 "Unhandled Exception") is why every rung keeps
refusing.

---

## Rung 2 — power + clocks + PMC_BOOT_0 (`ga10bprobe2`, this commit)

**Question.** With the GPU power domain provably ON and the `gpu@` node's DTB clocks enabled, does
the PMC block at BAR0+0 return an Ampere chip id — and, as by-products: which of those clocks did UEFI
leave running, and does BPMP let this kernel drive the GPU domain and clocks at all (every `err` on
the wire)?

**Code.** `unaos/crates/kernel/src/arch/aarch64/ga10b_probe.rs` tail (`ga10bprobe2_run`), knob
`ga10bprobe2` (`UNAOS_GA10B_PROBE2=1`, implies `tegra`), call site appended to the `tegra_early_stop`
BPMP-block line **before** rung 1's call. A sibling knob, not a dependent: rung 1's run is `-> !`
(SYSTEM_OFF unconditional), rung 2 **returns** so the boot continues to the desktop — the flight is a
full boot. Co-armed builds (both knobs) compile and would run rung 2 then rung 1's power-off; **the
rung-2 flight arms rung 2 alone.**

**Sequence (all announced before issued).**

1. DTB RAM walk (no MMIO): `gpu@` `reg[0]` → BAR0; `power-domains` odd word → domain id; `clocks`
   odd words → up to 8 BPMP clock ids, printed as found.
2. `MRQ_PG GET_STATE id` → `pg-before` (err, state). Timeout → `REFUSED reason=pg-timeout`, return.
3. Only if pre-state ≠ ON: `MRQ_PG SET_STATE id ON` (announced as the rung's first write) → err;
   2 ms settle.
4. `MRQ_PG GET_STATE id` again → `pg-readback` — **the explicit ON that alone earns the BAR0 read.**
5. Per clock: `MRQ_CLK IS_ENABLED` (before) → if 0: `MRQ_CLK ENABLE` → err (remembered as
   enabled-by-us). Then per clock `IS_ENABLED` (after). Census line
   `clocks: a of t running before, b of t after`. 2 ms settle.
6. If readback == ON: `about-to-read pmc_boot_0 reg=0x17000000`, one `read_volatile`, classified:
   - `0xFFFFFFFF` → `UNPOWERED reason=all-ones`
   - `0x00000000` → `UNPOWERED reason=zero-id`
   - `0xBADxxxxx` → `UNPOWERED reason=pri-error` (PRI fabric error pattern)
   - chipset field bits[28:20] = `0x17x` → `POWERED chipset=0x… arch=0x17 (Ampere) impl=0x… rev=0x…`
   - anything else → `POWERED chipset=… (NOT Ampere — first-class datum)`
   Else → `REFUSED reason=pg-on-refused | pg-readback-not-on`, no BAR0 touch.
7. Symmetric restore: `MRQ_CLK DISABLE` for every clock this rung enabled (reverse order); `MRQ_PG
   SET_STATE OFF` + `GET_STATE` only if this rung powered the domain on. `restored:` line, then
   `rung 2 complete — RETURNING`.

**Registers / mailboxes.**

| item | value | provenance |
|---|---|---|
| MRQ_PG 66; CMD_PG_SET_STATE 1, GET_STATE 2; PG_STATE_OFF 0 / ON 1 / RUNNING 2; request `{cmd, id[, state]}`, GET_STATE response payload[0]=state | — | Linux `include/soc/tegra/bpmp-abi.h` (SPDX `GPL-2.0 OR MIT`), fetched 2026-09-05; same wire `jb1c`/`jb5` prove on metal |
| MRQ_CLK 22; request word = subcmd[31:24] \| clk_id[23:0]; CMD_CLK_IS_ENABLED 6 (response payload[0] = 0/1), ENABLE 7, DISABLE 8 | — | same header; same wire `jb1c`/`jb7`/`clk_enable` prove on metal |
| GPU power domain id | 35 (DTB, EXT) | rung-1 wire; `TEGRA234_POWER_DOMAIN_GPU 35`, Linux `dt-bindings/power/tegra234-powergate.h` |
| GPU clock ids | DTB `gpu@` `clocks` (EXT, printed by the flight) | expected among `TEGRA234_CLK_GPC0CLK 41`, `GPC1CLK 236`, `GPUSYS 304`, `FUSE 40`, `GPU_PWR 42` (`dt-bindings/clock/tegra234-clock.h`) — **UNVERIFIED until printed**; the L4T `gpu@17000000` node's `clocks`/`clock-names` list was not available to this seat (UNKNOWN) |
| NV_PMC_BOOT_0 | BAR0 + `0x00000000`, R/O | NVIDIA `open-gpu-kernel-modules` `src/common/inc/swref/published/ampere/ga100/dev_boot.h` (MIT), fetched 2026-09-05 |
| BOOT_0 chipset field | bits[28:20] | `envytools` `rnndb/bus/pmc.xml` (NV10+ ID form, CHIPSET bits 20–28), fetched; `nouveau` nvkm derives chipset the same way (MIT) |
| Ampere architecture | 0x17 (upper bits of the chipset field) | `open-gpu-kernel-modules` `published/nv_arch.h` (MIT): `GPU_ARCHITECTURE_AMPERE 0x0170`, `GPU_IMPLEMENTATION_GA102 0x02`, `GA107 0x07` …, fetched |
| GA10B implementation | 0xB → chipset `0x17B` | **INFERRED** from the GA10x naming rule above (`nv_arch.h` carries no GA10B entry); the PASS predicate uses the architecture, the implementation is a datum |
| PRI error pattern | `0xBADxxxxx` (bits[31:20] = 0xBAD) | public (nouveau / open-gpu-kernel-modules PRI error handling) — recalled, not re-fetched this session; only used to LABEL a datum |

**Risk.** (a) `MRQ_PG SET_STATE` on the GPU domain: BPMP may refuse (`-EACCES`/`-EPERM` class, the
facts-file (a) pattern for MRQ_STRAP: "not permitted, proceed") — reported, the rung refuses the read
if the readback is not ON. Powering ON a domain UEFI left ON is idempotent; powering OFF is only ever
done if this rung powered it on. (b) `MRQ_CLK ENABLE`: BPMP refcounts; enabling a clock that is
already running is a no-op; the DISABLE at the end only touches clocks this rung enabled. (c) The BAR0
read: the SError risk of a PRI read on a rail that is ON but with a gated sys clock is **not bounded
by any public fact this seat holds** — the guard is the explicit `pg=ON` readback plus the clock
enables ordered BEFORE the read (they add clocks, never remove one), and the empirical bound is rung
1's five sane reads under UEFI's clock state. The read is announced; if it is the last line, the
capture names it. (d) A BPMP transaction hanging: `Chan::transfer` bounds every MRQ at 100 ms; the
announce line before each mutating MRQ names it.

**Witness / PASS-FAIL.** Extraction: `awk '/\[ga10bprobe2\]/' <log>`. Exactly one summary line
`[ga10bprobe2] pg=<state> clk=<n>/<t> boot0=<v> -> POWERED|UNPOWERED|REFUSED …`.
- **PASS** = `-> POWERED chipset=0x17b` (arch 0x17, impl 0xB) with `pg=0x1`, and the boot continues
  (a `[deskcascade]`/desktop line after the `rung 2 complete — RETURNING` line).
- **PASS-with-datum** = `-> POWERED` with arch 0x17 and impl ≠ 0xB (the naming inference was wrong;
  record the impl).
- **FAIL-informative** = `UNPOWERED` (any reason): the rail says ON but the PMC does not decode —
  the clock census (which of the DTB clocks were off, which ENABLE errs) is the next rung's input.
- **FAIL-refused** = `REFUSED reason=pg-*`: BPMP would not give us the domain — rung 2 repeats with
  the err code as the datum; a `-EACCES`-class refusal moves the ladder to "BPMP policy" (UNKNOWN —
  what MB2/UEFI configures BPMP to permit for the CCPLEX).
- **FAIL-fatal** = the last line is `about-to-read pmc_boot_0` (or a `SET_STATE`/`ENABLE` announce):
  that access is EL3-fatal in this state; the rung is re-run with that step removed and the datum
  recorded.

**Flight recipe.** `UNAOS_TEGRA=1 UNAOS_TEGRA_EL0=1 UNAOS_WITNESS=1 UNAOS_ORINRENDER=1
UNAOS_DESKCASCADE=1 UNAOS_ORINRX=1 UNAOS_HOLOCRON=1 UNAOS_ORINCLICK=1 UNAOS_GA10B_PROBE2=1 ./arroyo
esp-jetson` — **without `UNAOS_GA10B_PROBE1`** (rung 1 would power the board off after rung 2 ran).
Full boot; no cold-boot requirement (nothing is left changed); the desktop should come up behind it.

---

## Rung 3 — what the platform firmware left behind (read-only, one boot)

**Question.** Did MB2/UEFI stage anything for the GPU's secure boot — a boot-config (BCR) DMA
descriptor, a WPR/VPR region, a PMU image — and what do the remaining security fuses and the MC
engine enables say? This decides whether rung 4 starts from "nothing staged" (the expected case: rung
1's `br_retcode=0`) or from a partially-configured boot ROM.

**Reads (all BAR0-relative unless noted; every offset from the ACKED facts file).** Risk order:
fuses → MC → GSP falcon (v1) → GSP priscv BCR → PMU falcon2.

| register | offset | expect | provenance |
|---|---|---|---|
| fuse `opt_sec_debug_en` | 0x821040 | datum | facts (b) Security-state fuses |
| fuse `opt_wpr_enabled` | 0x8205ec | datum (WPR = the ACR's write-protected region exists?) | facts (b) |
| fuse `opt_vpr_enabled` | 0x82067c | datum | facts (b) |
| `mc_enable` | 0x000200 | datum: which engines UEFI left enabled | facts (b) Die-characterization |
| `mc_elpg_enable` | 0x00020c | xbar 0x4, l2 0x8, hub 0x20000000 bits | facts (b) |
| `top_device_info_cfg` | 0x0224fc | version_init = 0x2; the device_info2 table walk is rung 5's | facts (b) |
| GSP falcon `hwcfg` | 0x110000 + 0x108 | IMEM/DMEM sizes (a datum for rung 4's load) | facts (b) Legacy Falcon regs |
| GSP falcon `dmactl` | 0x110000 + 0x10c | require_ctx bit0 | facts (b) |
| GSP falcon `idlestate` / `irqmask` / `irqdest` | +0x04c / +0x018 / +0x01c | datum | facts (b) |
| GSP falcon `cpuctl` (v1) | 0x110000 + 0x100 | halt_intr bit4 (v1 view of "halted") | facts (b) |
| GSP priscv `bcr_ctrl` | 0x111000 + 0x668 | 0 expected (no BCR programmed: rung 1 `br_retcode=0`) | facts (b) RISC-V boot-ROM interface |
| GSP priscv `bcr_dmacfg` | 0x111000 + 0x66c | lock_locked bit31 — **if set, the BCR is locked by a prior boot and rung 4 cannot reprogram it this power cycle** | facts (b) |
| GSP priscv BCR DMA addrs | 0x111000 + 0x670..0x684 (pkcparam lo/hi, fmccode lo/hi, fmcdata lo/hi) | 0 expected | facts (b) |
| GSP priscv `boot_vector` lo/hi | 0x111000 + 0x380/0x384 | datum | facts (b) |
| GSP priscv `riscv_irqmask`/`irqdest` | 0x111000 + 0x528/0x52c | datum | facts (b) |
| PMU falcon2 `cpuctl` | 0x10b000 + 0x388 | halted bit4 (the PMU is a second RISC-V/falcon2 engine) | facts Aperture framing + (b) priscv cpuctl |

**Risk.** All on the rail rung 1 proved and rung 2 re-proves; the PMU falcon2 aperture (0x10b000)
is the one NEW address class — it goes last, announced. Priv-lockdown (rung 1: engaged) may make
priscv BCR reads return locked values: that is the datum, reported per register as `-UNREADABLE`.
No writes.

**Witness.** `[ga10bprobe3]`; PASS = the read list exhausted (last line `rung 3 complete`), with
`bcr_dmacfg` lock bit and `opt_wpr_enabled` recorded — both are inputs to rung 4's design.

## Rung 3b — the first GA10B MMIO write: engine reset + mailbox scratch (one boot)

**Question.** Does a GSP engine reset (assert → 10 µs → deassert) leave the falcon halted and
readable, and does a Falcon MAILBOX register hold a written value? This is the smallest possible write
that proves "this kernel can drive a GA10B engine register", and it is the rung where the
vendor-pad-SError class of failure is most likely to appear.

| step | register | value | provenance |
|---|---|---|---|
| assert | `pgsp_falcon_engine` 0x1103c0 bit0 | 0x1, hold ≥ 10 µs | facts (b) GSP engine reset (marked `[WRITE — probe omits]` for rung 1) |
| deassert | same | 0x0 | facts (b) |
| readback | GSP priscv `cpuctl` 0x111388 | halted bit4 = 1 expected after reset | facts (b) |
| scratch write | GSP falcon `MAILBOX0` 0x110000 + **0x040** | pattern 0x5A5AA5A5, read back | **PUBLIC-RECALLED**: the classic Falcon register map (nouveau `nvkm/falcon`, MIT; open-gpu-kernel-modules `dev_falcon_v4.h`, MIT) puts MAILBOX0/1 at +0x040/+0x044 — corroborated by the facts file's matching v1 offsets (irqmask 0x018, irqdest 0x01c, cpuctl 0x100, bootvec 0x104, hwcfg 0x108, dmactl 0x10c); **re-verify the pointer before import** |

**Risk.** A reset of the GSP engine while the BR priv-lockdown is engaged may be refused by the
PRI fabric (`0xBAD…` readback) or trap — the assert write is announced and stands alone as the boot's
one new write class; the mailbox write is skipped unless the post-reset `cpuctl` read is sane. Nothing
here touches the display, the fabric or any block outside BAR0. Restore: none possible for a reset
(the engine was halted and never-booted before; it is halted and never-booted after).

**Witness.** `[ga10bprobe3b]`; PASS = `mailbox0 wrote=0x5a5aa5a5 read=0x5a5aa5a5`.

## Rung 4 — the licence gate: booting NVIDIA's signed ACR/GSP image

**The wall, measured.** `opt_priv_sec_en = 1` (rung 1): production secure boot. The RISC-V boot ROM
only runs an image whose signature verifies against NVIDIA's key; the images are AES-encrypted and
PKC-signed (`orin-3d.md` §3, ga10b-clean-room.md §4). **No unsigned code can run on the GSP, PMU,
FECS or GPCCS of this die.** What CAN be done is to hand the boot ROM NVIDIA's own signed images — and
whether the public tree may stage those blobs, on media it builds, is **Peter's ruling** under
`CLEAN_ROOM_POLICY.md` §4 (the bunker rule). Until that ruling, rung 4 is design only.

**The blobs (named, not fetched, not embedded).** L4T r36.4.3 (JetPack 6.2) ships the GA10B microcode
under `/lib/firmware/nvidia/ga10b/` in the rootfs (`nvidia-l4t-firmware`). The families the boot
protocol below needs, by role: the **ACR** (Access Controlled Region bootstrapper — the first signed
image, sets up WPR and loads the rest), the **GSP** RISC-V image and its **FMC** ("first mutable
code", the BCR's `fmccode`/`fmcdata` + `pkcparam` triple names it) plus manifest, the **PMU**
image, and the **FECS/GPCCS** GR context-switch falcon images (rung 6). **Exact filenames: UNKNOWN to
this seat — enumerate with `ls /lib/firmware/nvidia/ga10b/` on a JetPack 6.2 rootfs; the list goes to
Peter with the licence question.** The NVIDIA firmware licence (the L4T "License For Customer Use of
NVIDIA Software"/ the firmware EULA) is the document to rule on.

**The load protocol in FACTS (the boot-ROM handshake, facts (b) SEQ — the order is the fact):**

1. Place the signed FMC code, FMC data and PKC-parameter blobs in DRAM the GPU can DMA (physically
   contiguous; the target aperture is `bcr_dmacfg.target_noncoherent_system = 0x2`).
2. Write the BCR DMA addresses: `pkcparam` lo/hi 0x670/0x674, `fmccode` lo/hi 0x678/0x67c, `fmcdata`
   lo/hi 0x680/0x684 (priscv, falcon2-base-relative).
3. `bcr_dmacfg` 0x66c = target_noncoherent_system (0x2) | lock_locked (0x80000000).
4. `bcr_ctrl` 0x668 = 0x111 (brom_config path) — or 0x11 (the alt set_bcr path).
5. Optional: `riscv_boot_vector` lo/hi 0x380/0x384.
6. `priscv_cpuctl` 0x388 = startcpu_true (0x1) — **the boot ROM runs.**
7. Poll `br_retcode` 0x65c result[1:0]: PASS = 0x3, FAIL = 0x2 (0x0/0x1 = still running).
8. Halt state: `priscv_cpuctl` bit4; priv-lockdown state: `falcon_hwcfg2` bit13 (expected to drop
   once a verified image is running — a datum).
9. GSP RPC ring / message queues after PASS: **UNKNOWN — Group A pass** (the GSP↔CPU command/message
   queue layout, the ACR's WPR descriptor and the "boot the other falcons" sequencing are nvgpu
   facts this seat did not extract).

Also needed before step 1: the static power-gate straps — facts (a): on silicon the PG straps are
BPMP's, software must NOT program fuse straps itself; MRQ_STRAP {cmd=STRAP_SET, id, value} ids
OPT_GPC 1 / OPT_FBP 2 / OPT_TPC_GPC0 3 / OPT_TPC_GPC1 4, with `-BPMP_ENODEV`/`-BPMP_EACCES` = proceed.
Whether that MRQ is even permitted to the CCPLEX from our boot is a rung-4a datum (one boot, one
MRQ_STRAP query, `err` on the wire).

**PASS** = `br_retcode` result == 0x3 after step 6 with an NVIDIA-signed image. **FAIL** = 0x2 (the
image or its placement was rejected — the ROM's reason code, if any, lives in `br_retcode`'s upper
bits: UNKNOWN). The rung is one boot per attempted image.

## Rung 5 — the first copy-engine job (host FIFO)

**Question.** Can this kernel build one channel, one GPFIFO, one pushbuffer with a CE method stream,
put the channel on a runlist, and see a DMA copy land in a readback buffer? This is the first "the
GPU did work for us" rung, and whether it needs rung 4 first is itself **UNKNOWN**: on a dGPU the CE
runs without the GSP-RM, but on secure GA10B the host/CE PRI blocks may sit behind the ACR's
priv-lockdown release (rung 1 found lockdown engaged). Rung 3's `mc_enable` datum and rung 4's
lockdown-after-PASS datum decide it.

**Facts needed (UNKNOWN — Group A pass):** PFIFO/PBDMA/runlist register offsets for Ampere Tegra,
the RAMFC (channel context) layout, USERD, the runlist entry format, the `device_info2` table walk
(facts (b): `top_device_info_cfg` 0x0224fc, `version_init = 0x2`, an indexed table) that yields the
CE's PRI base and runlist id, the CE PRI block, and the instance-block / page-table (MMU) format the
channel needs (GPU virtual addressing is unavoidable: CE methods take GPU VAs). Public facts that DO
exist: the class numbers — Ampere DMA copy `AMPERE_DMA_COPY_B = 0xC7B5` (GA10x; GA100 uses 0xC6B5),
`AMPERE_CHANNEL_GPFIFO_A = 0xC56F` — from `open-gpu-kernel-modules` class headers (MIT) /
nouveau (MIT), recalled; the copy-class method layout (offset-in/out lo/hi, line length, launch DMA)
is documented publicly for prior generations (envytools) and is stable across them — **re-verify per
class before import**. The oracle: a 4 KiB pattern copied and compared byte-for-byte.

**Risk.** Runlist submission on a misconfigured channel hangs the host or faults the MMU — every
step is its own boot (JX3), and the MMU fault registers are read (not the display) to name the fault.

## Rung 6 — 3D: one triangle through the GR pipe

**Question.** With FECS/GPCCS running (rung 4's signed images — there is no GR without them on this
die), a golden context image obtained, and the GR engine reset/initialised, does an `AMPERE_B`
(`0xC797`, recalled, MIT class headers) 3D method stream rasterise one triangle into a render target
that matches the `rast` crate's reference (the tree's standing oracle: "GPU output == rasterizer
reference", `orin-3d.md` §4.3)?

**Facts needed (UNKNOWN — Group A pass):** GR init sequence (the largest item: SM/TPC/GPC config, the
context image size and layout the FECS reports, the golden context creation handshake with FECS,
the zcull/pm/… sub-contexts), the 3D class's method set for a minimal pipeline (render target,
viewport, vertex attribute, shader program upload — which needs a SASS/GA10B ISA assembler: NO public
spec; **this is a second wall independent of the firmware one**), and the L2/FB config. What IS
public: the class numbers, the pushbuffer/method encoding (nouveau, MIT), and the compute class
(`AMPERE_COMPUTE_B 0xC7C0`, recalled) — a compute kernel is the smaller first 3D-class rung.

**The honest end of the ladder.** Rungs 5–6 need (a) Peter's blob ruling (rung 4) and (b) at least
one more quarantined Group A facts pass sized like the rung-1 one but 10× larger, by a seat that will
not write the code. The blob-free ceiling (`orin-3d.md` §3) — CPU rasterisation into the inherited
scanout — is not moved by any rung below 4.

---

## Provenance table (every fact this ladder imports, with its source and §6 terms note)

| fact | source | §6 terms note |
|---|---|---|
| GSP falcon base 0x110000; falcon2/priscv base 0x111000; PMU falcon2 base 0x10b000 | ACKED facts file, Aperture framing | extracted 2026-08-25 under §6, independently reviewed (ACK-WITH-EDITS applied) |
| fuse opt_priv_sec_en 0x820434, opt_sec_debug_en 0x821040, opt_wpr_enabled 0x8205ec, opt_vpr_enabled 0x82067c | facts (b) Security-state fuses | same |
| falcon v1 regs irqmask 0x018, irqdest 0x01c, idlestate 0x04c, cpuctl 0x100 (startcpu bit1, halt_intr bit4), bootvec 0x104, hwcfg 0x108, dmactl 0x10c (require_ctx bit0), hwcfg2 0x0f4 bit13 | facts (b) Legacy Falcon regs | same |
| priscv cpuctl 0x388 (startcpu 0x1, halted bit4), br_retcode 0x65c (result[1:0], FAIL 2 / PASS 3), bcr_ctrl 0x668 (0x111 / 0x11), bcr_dmacfg 0x66c (target_noncoherent_system 0x2, lock_locked 0x80000000), BCR DMA addrs 0x670–0x684, riscv_irqmask 0x528, riscv_irqdest 0x52c, boot_vector 0x380/0x384 | facts (b) RISC-V boot-ROM interface + handshake SEQ | same |
| pgsp_falcon_engine 0x1103c0 assert 0x1 / deassert 0x0, 10 µs | facts (b) GSP engine reset | same (marked WRITE) |
| top_num_gpcs 0x022430 [4:0]; top_device_info_cfg 0x0224fc, device_info2 indexed, version_init 0x2; mc_enable 0x200; mc_elpg_enable 0x20c (xbar 0x4, l2 0x8, hub 0x20000000) | facts (b) Die-characterization | same |
| MRQ_STRAP ids OPT_GPC 1 / OPT_FBP 2 / OPT_TPC_GPC0 3 / OPT_TPC_GPC1 4; straps are BPMP's on silicon; ENODEV/EACCES = proceed | facts (a) | same |
| MRQ_PG 66 / MRQ_CLK 22 and their subcommands, states, request/response shapes | Linux `include/soc/tegra/bpmp-abi.h`, SPDX `GPL-2.0 OR MIT` (fetched 2026-09-05) | permissive (MIT option); the tree's `bpmp_tegra.rs` already cites it |
| TEGRA234_POWER_DOMAIN_GPU 35; TEGRA234_CLK_GPC0CLK 41, GPU_PWR 42, GPC1CLK 236, GPUSYS 304, FUSE 40 | Linux `dt-bindings/power/tegra234-powergate.h`, `dt-bindings/clock/tegra234-clock.h` (fetched) | numeric ids only — the same class of citation `bpmp_tegra.rs` uses for CLK_PWM3/RESET_PWM3; the live DTB is the authority at runtime |
| NV_PMC_BOOT_0 = 0x0 (R/O) | NVIDIA `open-gpu-kernel-modules` `published/ampere/ga100/dev_boot.h`, MIT (fetched) | permissive |
| BOOT_0 chipset field bits[28:20] | `envytools` `rnndb/bus/pmc.xml` (fetched); nouveau nvkm (MIT) | permissive / documentation |
| Ampere architecture 0x0170 → 0x17; GA10x implementation nibble rule | `open-gpu-kernel-modules` `published/nv_arch.h`, MIT (fetched); GA10B = 0xB **inferred** | permissive; the inference is labelled in code and on the wire |
| PRI error pattern 0xBADxxxxx | nouveau / open-gpu-kernel-modules (recalled) | label only; re-verify before it gates anything |
| Falcon MAILBOX0/1 +0x040/+0x044 | nouveau `nvkm/falcon` / `dev_falcon_v4.h` (MIT) — recalled | rung 3b; re-verify the pointer before import |
| class numbers 0xC56F / 0xC7B5 / 0xC797 / 0xC7C0 | open-gpu-kernel-modules class headers / nouveau (MIT) — recalled | rungs 5–6; re-verify before import |
| rail ON at handoff; BAR0 0x17000000; pd 35; fuse/hwcfg2/br_retcode/cpuctl/gpcs values | rung-1 flight readback (o3d) | measured |
| L4T GA10B firmware set under `/lib/firmware/nvidia/ga10b/`; exact filenames | UNKNOWN (not enumerated) | rung 4 — Peter's ruling; never fetched |
| GSP RPC/message-queue layout, ACR WPR descriptor, PFIFO/PBDMA/runlist/RAMFC/USERD, GPU MMU page-table format, GR init, SASS ISA | UNKNOWN — Group A pass (or, for the ISA, no public source at all) | rungs 4–6 |

**§6 terms note.** No nvgpu text, macro, struct or comment appears in this file or in
`ga10b_probe.rs`; every ACKED-facts entry above is a pointer-backed offset/bit/constant/ordering from
the reviewed facts file, and every non-facts-file entry is either a live-DTB/flight measurement or a
permissively-licensed public header. Items marked "recalled" were written from memory of public
sources and are not load-bearing for rung 2's code; they must be pointer-verified before any rung
imports them.
