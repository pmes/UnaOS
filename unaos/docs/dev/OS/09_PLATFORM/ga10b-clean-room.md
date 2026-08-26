# GA10B clean-room fact pipeline and the first read-only probe rung

This note is the mechanics companion to [`orin-3d.md`](orin-3d.md) §3 ("The GPU rung —
GA10B, and what it would actually cost") and to
[`CLEAN_ROOM_POLICY.md`](../../../../../docs/MANIFESTO/CLEAN_ROOM_POLICY.md) §6. `orin-3d.md`
states *what* GA10B bring-up costs and that the licensing decision is Peter's; the policy §6
records his 2026-08-25 adjudication. This note describes *how* facts move from NVIDIA's GPL
`nvgpu` driver into the tree without any expression crossing, and proposes the first probe rung.

No kernel code is introduced by this arc. What follows is a design and a facts-import area.

## 1. The pipeline

The GA10B iGPU carries no Group-A-legal specification we can rely on: there is no public TRM at
register granularity, and envytools/rnndb do not cover Ampere Tegra (`ga10b`). The facts needed
to *observe* the block therefore come from NVIDIA's own GPL-2.0 `nvgpu` driver, consumed under
the quarantined clean-room terms of policy §6.

```
  quarantine checkout            extraction pass              terms review            import
  (outside the repo)      →   (Group A reader emits    →   (independent seat    →   (reviewed
  GPL nvgpu, r36.x            facts + file:line            re-checks against         facts file
  matched to the Orin        pointers; no code,           §6 terms; COI guard,      only, into
  BSP; provenance +          no prose, no                 ack recorded)             the tree's
  checksum recorded)         expression)                                            facts area)
```

Concretely, for this arc:

1. **Quarantine checkout.** `~/unaos-bench/scratch/quarantine/nvgpu/` — L4T **r36.4.3**
   (JetPack 6.2) `nvgpu`, extracted from NVIDIA's public `public_sources.tbz2`
   (`kernel_oot_modules_src.tbz2` → `nvgpu/`). Source URL, release, and tarball sha256 are in
   `quarantine/PROVENANCE.txt`. This directory is strictly outside the repo; nothing under any
   worktree copies its text.
2. **Extraction pass.** A Group A reader consumes the GPL source and emits a facts file whose
   only content is register offsets, bit-field layouts, magic constants, and required ordering,
   each tagged with an `nvgpu:file:line` provenance pointer. Tag vocabulary: `FACT` (offset /
   bit / constant), `SEQ` (ordering), `EXT` (fact from outside nvgpu — the DTB/TRM), `NOTE`
   (extractor interpretation, not a source fact).
3. **Terms review.** The facts file is reviewed against §6: offsets/bits/constants/ordering with
   pointers only, no expression. The review is the conflict-of-interest guard — the extractor
   does not clear its own import; an independent seat re-checks and the ack is recorded in the
   import commit.
4. **Import.** Only the reviewed facts file enters the tree, under the facts-import area
   [`ga10b-facts/`](ga10b-facts/). Raw quarantine working notes stay in quarantine.

**Group boundary.** Whoever ran the extraction is **Group A** for GA10B and may not also author
the UnaOS GPU implementation of it (policy §2). The reviewed facts file is the Group A → Group B
handoff.

## 2. Separate probe media — the standing rule for GA10B flights

GA10B probing does **not** ride the desktop image. It follows the same separation the V3D probes
adopted: each probe is its **own staged boot image**, built from a clean tree, carrying only the
one probe under test. This keeps a fatal probe (see `orin-3d.md` §4.1 — a read of a powergated
Tegra partition is EL3-fatal) from taking down a working desktop boot, and keeps each flight's
capture unambiguous.

The discipline, inherited from the JX2/JX3 display-handoff work:

- **One register-step per boot (the JX3 model).** A probe image advances the GA10B state machine
  by exactly one observable step per boot — one new MMIO address class touched, or one BPMP
  transaction — never a sweep. The last serial line names the address about to be touched, so if
  the boot dies, the capture names the killer.
- **Announce-first.** Emit a serial announce line *before* touching any new MMIO address class or
  issuing any BPMP MRQ. Witness family names use tokens **> 8 bytes** (tokens ≤ 8 bytes can be
  LLVM-immediate-encoded and become invisible to artifact `grep` while fully working — orin-6
  §7).
- **Power gate proven first.** Never read a GA10B aperture until a BPMP MRQ proves the GPU
  power-domain is on (`orin-3d.md` §4.1). The power query is a BPMP transaction, not a GPU read.
- **Cold-boot ending = machine OFF.** Per the 2026-08-25 bench law, a flight whose next boot must
  be cold ends by powering the board **off** (aarch64 PSCI `SYSTEM_OFF`), not idling and not
  warm-rebooting. A powered-off board is the "ready for cold boot" signal that survives the
  monitor sleeping. The shutdown verb is being built in parallel (exec-reboot); until it lands,
  design against `SYSTEM_OFF` and stub the call, ending the flight at a labelled halt so the
  operator can cut power.

## 3. The first probe rung (proposal — read-only, one page)

**Goal.** Read-only state observation of GA10B: is the rail on, has the GSP RISC-V core ever
booted, and is the block priv-locked? No firmware boot, no write to any engine. This answers "how
dead is it, and in what way" before any bring-up spend, and it exercises the announce/gate/OFF
discipline end to end. All register facts below are sourced in
[`ga10b-facts/ga10b-probe-rung1.facts.md`](ga10b-facts/ga10b-probe-rung1.facts.md).

**Registers, in strict order (all reads):**

1. **BPMP power-domain query** for the Tegra234 GPU domain (MRQ) — assert the rail is on. If the
   domain is off or the MRQ is unpermitted, verdict `GA10B-RAIL-GATED` and stop (do **not** touch
   BAR0). The domain id is a DTB/BPMP-ABI constant (`EXT`), resolved from the Orin FDT
   `power-domains` phandle, never guessed.
2. **`fuse_opt_priv_sec_en`** (BAR0 `0x820434`) — is production secure boot fused? Expected `1`
   on this silicon. Records the wall.
3. **`falcon_hwcfg2`** (GSP falcon base `0x110000` + `0x0f4`), bit 13 — is BR priv-lockdown
   engaged? Expected engaged on secure silicon; if engaged, priscv reads below may return locked
   values, which is itself the datum.
4. **`priscv_br_retcode`** (GSP falcon2 base `0x111000` + `0x65c`), result bits[1:0] — the
   boot-ROM verdict. `0x3`=pass, `0x2`=fail, `0x0/0x1`=BR never reached a verdict.
5. **`priscv_cpuctl`** (`0x111000` + `0x388`), bit 4 (halted) — is the RISC-V core halted?
6. **`top_num_gpcs`** (BAR0 `0x022430`), bits[4:0] — die identity cross-check (GA10B Orin Nano =
   2 GPC).

**Expected values per state:**

| register | powered-off (rail gated) | powered-on, never-booted GPU (our case) |
|---|---|---|
| BPMP GPU domain | off / unpermitted → STOP | on |
| `opt_priv_sec_en` | not read | `1` (secure fused) |
| `hwcfg2` bit13 | not read | `1` (priv lockdown engaged) |
| `br_retcode[1:0]` | not read | `0x0` (BR never ran — no GPU fw is loaded by MB2) |
| `cpuctl` halted bit4 | not read | `1` (core halted) |
| `top_num_gpcs` | not read | `2` |

The `br_retcode == 0x0` reading is the load-bearing expectation: `orin-3d.md`/state records that
MB2 loads no GPU binary and there is no `A_gpu-fw` partition, so the GSP RISC-V boot ROM should
never have reached a pass/fail verdict. A `0x2`/`0x3` here would contradict that and is a
first-class finding.

**Verdict vocabulary** (each a distinct serial line, tokens > 8 bytes):
`GA10B-RAIL-GATED`, `GA10B-RAIL-POWERED`, `GA10B-SECURE-FUSED`, `GA10B-PRIVLOCK-ENGAGED`,
`GA10B-BROM-NEVERRAN`, `GA10B-BROM-PASSED`, `GA10B-BROM-FAILED`, `GA10B-CORE-HALTED`,
`GA10B-GPC-CENSUS=<n>`.

**Witness family name:** `GA10BPROBE1` (> 8 bytes), with lines of the form
`:: GA10BPROBE1: <verdict> reg=<addr> val=0x******** ::`.

**Media / knob shape:** its own staged image, never the desktop card. Gate `UNAOS_GA10B_PROBE1=1`
(off by default; the knob must appear in the `⚡ kernel features:` banner and be `grep`-proven
reachable in the artifact, not merely compiled). The probe runs after the BPMP MRQ gate, emits
its six announce/verdict pairs, then — since the next boot must be cold — ends in PSCI
`SYSTEM_OFF` (stubbed to a labelled halt until exec-reboot's verb lands). One register-step per
boot still applies: rung 1 reads the set above in a single flight because they are all in
already-safe apertures (fuse, GSP falcon, top) *once the rail is proven*; the first flight that
reaches a **new** aperture class beyond these advances by one step only.

**Implementation status (2026-08-25):** the proposal above is now IN THE TREE as
[`arch/aarch64/ga10b_probe.rs`](../../../../crates/kernel/src/arch/aarch64/ga10b_probe.rs)
(`ga10bprobe1_run`), gated behind the `ga10bprobe1` Cargo feature (`UNAOS_GA10B_PROBE1=1`, implies
`tegra`) and wired as an appended, `#[cfg]`-erased call in `tegra_early_stop`'s BPMP block — so the
disarmed jetson image stays byte-identical to baseline. The BAR0 base and power-domain id are
resolved from the firmware DTB `gpu@` node (EXT), never hardcoded. Two adjustments to the
proposal, made in code and reflected here: the witness family is emitted BRACKETED (`[ga10bprobe1]`)
to match the tree's other Orin witness families and to stay well over the 8-byte LLVM
immediate-encode floor; and the run ends in the REAL `power::shutdown()` (PSCI `SYSTEM_OFF`, in tree
since 38d95900), not the stub — exec-reboot's verb has landed. The armed polarity is type-checked by
the `arm-tegra-ga10bprobe1` leg of `KERNEL_CFG_MATRIX`.

## 4. What this rung does not do

It boots no firmware, writes no engine register, and asserts no reset. The write-path facts in
the facts file (BCR config, `cpuctl` startcpu, GSP engine reset) are recorded so the read-only
probe knows what a *completed* boot would look like — they are not exercised. Firmware boot on
production GA10B remains gated by AES-encrypted, PKC-signed ucode with boot-ROM enforcement
(`OPT_PRIV_SEC_EN` fused); that wall is unchanged by this arc and is out of scope.
