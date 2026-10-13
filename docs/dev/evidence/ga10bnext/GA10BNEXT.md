# GA10BNEXT — which rung is the next UNEXECUTED one, and the register sequence it needs (2026-10-14)

Read first (R106, one hour): `docs/dev/OS/orin-queue.md` (every GA10B entry, 2026-09-12 to 2026-09-15),
`docs/dev/OS/08_VIDEO/GA10B-RUNG4-BRIEF.md`, `GA10B-RUNG5-BRIEF.md` (§1.4, §2.2, §4.1.1, §8, §9),
`GA10B-RUNG6-BRIEF.md`, the code `unaos/crates/kernel/src/arch/aarch64/ga10b_{probe,fw,ignite}.rs`, the
ACKED facts file `unaos/docs/dev/OS/09_PLATFORM/ga10b-facts/ga10b-probe-rung1.facts.md`, and the six flown
wires under `docs/dev/evidence/orin27/` and `orin28/`. Peter, 2026-10-14: "what about the GPUs wifi and there
must be more!!!" — R101: two GPU slots always, each refilled with the next rung; the GA10B ladder has not moved
since 2026-09-15 (A95).

## 1. Where the ladder stands, from its own record

| rung | knob | what it varied | verdict on the wire | where |
|---|---|---|---|---|
| 4b | `UNAOS_GA10B_PROBE4=2` | our blob-free image, raw addresses, BRFETCH-DMA door | `br_retcode=0x00000002` | `orin27/render13-boot2.log` |
| 4e | `=5` (and `=7` deferred) | addresses `pa >> 8` | `0x00000002` (raw and shift IDENTICAL) | `orin28/render14-boot3-shutdown-rung4e.log` |
| 4f | `=6` | `bcr_ctrl 0x011` (BRFETCH FALSE), no port loads | `0x00000002`; `bcr_ctrl` held `0x11` — BRFETCH FALSE is a value the register ACCEPTS | `orin28/render15-4f-brfetch.log` |
| 5a | `UNAOS_GA10B_PROBE5=1` | NVIDIA's signed `acr-gsp` triple, canonical map | `0x00000002` | `orin28/render15-5a-vendor-ignition.log` |
| 5s | `=3` | the `safety-scheduler` triple | `0x00000002`, twice, byte-identical | `orin28/render15-5s-boot{1,2}-safety-scheduler.log` |
| 5m | `=4` | code/data SWAPPED | `0x00000002` | `orin28/render15-5m-swapped-map.log` |
| 5b | `=2`/`=7` | read-only ACR→PMU oracles | never reached: gated on `ACR-ACCEPTED`, which never occurred | built, unflown (A65) |
| 6a–6d | — | PMU, FECS/GPCCS, golden context, a channel | DESIGN only, each blocked on the rung below | RUNG6 brief §2 (A65 `open`) |
| GA10B7 | — | the written finding | "(b) NO BOOT — the route is closed at this altitude" | RUNG5 brief §9 (A95 `dropped`) |

Unchanged across all six ignitions: `hwcfg2 lockdown=1 (raw=0x0001a733)`, `gsp_falcon_cpuctl_v1 = 0xbadf5620`,
`lock_latched=1 post_lockdown=1 v1_readable=0 halted=1`, `mbox-arg-written=none mbox-post-read=0x00000000`.

## 2. The next UNEXECUTED rung, by the ladder's own logic — confirmed, with one correction

**The queue's claim** (2026-09-15, "WHAT THE WIRE ITSELF POINTS AT"): nvgpu names TWO doors — the BRFETCH-DMA
door every rung flew, and the GSP-ACR door (`MAILBOX0/1` descriptor + `bcr_ctrl 0x11` + IMEM/DMEM PORT
LOADS), "a route the ladder has never taken". **Confirmed from the briefs:** RUNG5 §9.2 (GA10B7) decodes
`bcr_ctrl 0x11` as `BRFETCH = FALSE` (§2.2, against the GA102 MIT header) and states that door 2 "requires the
image resident in IMEM/DMEM before STARTCPU — a write class the ladder has never used"; rung 4f flew
`bcr_ctrl=0x11` WITHOUT the port loads, so its `0x2` is expected and attributes nothing. RUNG6 §2–§4 are all
downstream of `ACR-ACCEPTED` and cannot run. **So the only rung with no measurement behind it is door 2.**

**The correction, and it is the reason this arc builds anything:** GA10B7 ruled door 2 "not a candidate"
under the rung-5 brief's rule *"a door is not a candidate until its write list exists"* (§9.2), because the
GA10B IMEM/DMEM ACR write list is in no public source the seat could reach — and this seat could not reach
it either (HEAD: nv-tegra is blocked; nvgpu on github raw is gk20a-era). That rule bars the door's
IGNITION. It does not bar the door's CENSUS. The ladder's OLDER rule is that a write class is censused before
it is used — rung 3 read before 3b wrote (RUNG3), rung 4a write-readback-RESTORED the seven BCR registers
before 4b ignited through them (RUNG4 brief §3.2), rung 5b reads before 6a would write (RUNG6 §3 "the same
read-only oracle before the write rung discipline"). Door 2's write class is the falcon IMEM/DMEM PORT
(`IMEMC/IMEMD/IMEMT/DMEMC/DMEMD`), and **no rung has ever read one of those registers on this die**. Its
census needs no write list from nvgpu: the port registers' offsets and fields are PUBLIC (nouveau
`nvkm/falcon/v1.c`, MIT; open-gpu-kernel-modules `dev_falcon_v4.h`, MIT), the same provenance class rung 3b's
`mailbox0` carries, and the census is write-readback-restore of a control register plus ONE WORD through a
data port — no BCR write, no lock, no STARTCPU, nothing spent.

**Why its answer is not the `0x2` oracle (R53's condition — a boot whose FAIL teaches what six FAILs did
not).** The wire already shows priv-lockdown is per-register inside the GSP legacy-falcon block at
`BAR0+0x110000`: `hwcfg @0x108 = 0x80420100`, `hwcfg2 @0x0f4 = 0x0001a733`, `mailbox0 @0x040 = 0x00000000`
read as VALUES, while `dmactl @0x10c`, `idlestate @0x04c`, `irqmask @0x018`, `irqdest @0x01c`, `cpuctl
@0x100` read `0xbadf5620` (pri-error) — every flight, e.g. `render15-5m-swapped-map.log` pre-ignition
census. The ports at `0x180`/`0x1c0` are in that block and on neither list. A `pri-error` on them says the
lockdown REACHES the door's write class — door 2 closed at this altitude by direct measurement, R19 "failed
under lockdown=1, EL2, this die". A held round-trip says the write class is OPEN under lockdown — a GPU-side
positive no previous flight could show, and the measured floor the door's ignition rung would stand on. Either
answer is attributable, which `br_retcode` never was (§9.1).

**A datum already on the wire, decoded here for the first time:** `hwcfg = 0x80420100`. With the public
HWCFG layout (nouveau `nvkm/falcon/base.c`: IMEM_SIZE bits [8:0], DMEM_SIZE bits [17:9], 256-byte blocks)
that is IMEM 0x100 blocks = 64 KiB and DMEM 0x100 blocks = 64 KiB. `acr-gsp.text` is 28,672 B and
`acr-gsp.data` 9,472 B (A61) — the triple FITS a port-load. PUBLIC-RECALLED, NOT in the ACKED facts file; the
rung prints the decode beside the raw value so a wrong field layout is visible on the wire.

## 3. Rung 5g — the GSP-ACR door, step one: the IMEM/DMEM PORT CENSUS

Knob: `UNAOS_GA10B_PROBE5=6` → features `ga10bprobe5a` + `ga10bprobe5g` (the digit 6 is free: 1/5/2/7/3/4 are
taken; no `d` form — there is no ignition to defer). Code: `unaos/crates/kernel/src/arch/aarch64/ga10b_port.rs`,
a child module declared from `ga10b_ignite.rs` under `#[cfg(feature = "ga10bprobe5g")]`; `ga10bprobe5_run`
runs it FIRST and RETURNS — on a `=6` build the loader and the ignition do not run, nothing is spent, the boot
continues into the desktop. Witness family `[ga10bprobe5g]`; verdict `:: ga10b: 5g port-census … ::`.

### 3.1 The exact register sequence (BAR0 from the DTB `gpu@` node; GSP legacy-falcon block = BAR0 + 0x110000)

| step | register | offset | access | provenance | what it tells |
|---|---|---|---|---|---|
| bracket | BPMP MRQ_PG(66) GET/SET_STATE id from `gpu@ power-domains`; MRQ_CLK IS_ENABLED/ENABLE per `clocks` | — | BPMP, not MMIO | rung 2/4a (facts §SEQ) | rail ON with readback or `DEFERRED reason=pg-*`, zero MMIO |
| R1 | `gsp_falcon_hwcfg` | `0x108` | read | ACKED facts (`hw_falcon_ga10b.h:95`) | IMEM/DMEM sizes (decode PUBLIC-RECALLED) |
| R2 | `gsp_falcon_hwcfg2` | `0x0f4` | read | ACKED facts, bit 13 | lockdown engaged (expected 1) |
| R3 | `gsp_falcon_mailbox0` | `0x040` | read | PUBLIC-RECALLED, metal-proven rung 3b | the FMC channel, expected 0 |
| R4 | `gsp_falcon_cpuctl_v1` | `0x100` | read | ACKED facts | the lockdown CONTROL: expected `pri-error`; a value is itself a finding |
| R5 | `gsp_falcon_mailbox1` | `0x044` | read, FIRST TOUCH | PUBLIC-RECALLED (`dev_falcon_v4.h`) | door 2's descriptor-hi register answers or not |
| R6–R10 | `imemc0 0x180`, `imemd0 0x184`, `imemt0 0x188`, `dmemc0 0x1c0`, `dmemd0 0x1c4` | | read, FIRST TOUCH | PUBLIC-RECALLED (nouveau `nvkm/falcon/v1.c`: `0x180 + port*16`, `0x1c0 + port*8`) | the write class: value or pri-error |
| C1 | `imemc0` | `0x180` | write `0x00000100` (BLK=1), readback, write as-found, readback | fields OFFS [7:2] BLK [23:8] AINCW 24 AINCR 25 SECURE 28 (nouveau v1.c) | `held` / `not-held` / `unreadable`; skipped if R6 was pri-error |
| C2 | `dmemc0` | `0x1c0` | same | same | same |
| D1 (only if C1 `held`) | `imemc0` ← 0 then `imemd0` read (as-found); `imemc0` ← 0, `imemd0` ← `0x4A10B5D0`; `imemc0` ← 0, `imemd0` read; `imemc0` ← 0, `imemd0` ← as-found; `imemc0` ← 0, `imemd0` read | `0x180`/`0x184` | 5 control writes, 2 data writes, 3 data reads | the port is re-pointed before EVERY access; no auto-increment relied on | `held` iff the pattern read back AND the restore read back |
| D2 (only if C2 `held`) | the same through `dmemc0`/`dmemd0` | `0x1c0`/`0x1c4` | | | |
| restore | clocks this rung enabled DISABLED in reverse; MRQ_PG OFF iff this rung powered ON | — | BPMP | rung 4a's symmetric restore | the board left as found |

Every access is announced on the line before it (`about-to-read` / `about-to-WRITE … if this is the LAST
line …`); every write has exactly one result line (a data-port write's result is the value written, its
readback the next announced read through the re-pointed port), so the orin28 announce/result counter
balances in this family.

### 3.2 Verdict vocabulary (one `[ga10bprobe5g] port census: … -> <arm>` line, then the `::` line)

| arm | `:: ga10b: 5g … ->` | meaning |
|---|---|---|
| `PORT-OPEN` | `PASS` | both data ports round-trip under lockdown — door 2's write class is OPEN; the door's ignition rung has a floor (and still needs the nvgpu write list, RUNG5 §9.5 item 1, before it is a candidate) |
| `PORT-GATED` | `FAIL reason=port-gated` | all five port registers pri-error while hwcfg/hwcfg2 answer — lockdown reaches the ports; door 2 closed at this altitude by measurement |
| `PORT-PARTIAL` | `FAIL reason=port-partial` | one port open, the other not |
| `PORT-CTRL-NOT-HELD` / `PORT-DATA-NOT-HELD` | `FAIL reason=port-ctrl-not-held` / `port-data-not-held` | a register answers but a write does not hold |
| `PORT-UNREADABLE` | `FAIL reason=port-unreadable` | hwcfg itself did not answer — not the block rungs 4c/5a read; re-fly cold |
| — | `DEFERRED reason=no-gpu-node|no-power-domains|pg-timeout|pg-on-refused|pg-readback-not-on` | not measured, zero MMIO, nothing spent |
| (QEMU virt) | `SKIP reason=no-ga10b` | printed by the loader fixture (`ga10b_fw.rs::fixture_service`, the only GA10B verdict a virt boot has) on any build without `ga10bprobe5a` |

### 3.3 What rung 5g does NOT do

No vendor byte is read (the loader does not run on `=6`); no BCR register is written; no `bcr_dmacfg` lock; no
`bcr_ctrl`; no STARTCPU; no `MAILBOX0/1` write (RUNG5 §9.3 closed that question — the mailbox is the FMC's
channel, downstream of the ROM's verdict); no PMU/FECS/GPCCS; the power cycle is not spent and the board is
restored. The only state it changes and restores is one word of GSP IMEM and one word of GSP DMEM — RAM of a
halted falcon (`priscv_cpuctl halted=1` on every flight) with no image loaded.

## 4. Owed to the fold (EXECUTOR-BRIEF head 3: files outside the arc's list — REPORTED, NOT MADE)

1. `unaos/crates/kernel/Cargo.toml`, appended after the `ga10bprobe5m` entry:
   `ga10bprobe5g = ["ga10bprobe5a"]` (with its comment block, the `ga10bprobe5m` shape). Without it the arroyo
   mapping `6)` and the `arm-tegra-ga10bprobe5g` leg name a feature cargo does not know.
2. `unaos/scripts/banner-cert.sh`, a row (the arm media path calls `banner_cert_arm`, and a banner feature
   with no row is exit 2 NO VERDICT): `ga10bprobe5g|[ga10bprobe5g] rung 5g ARMED|tegra|measured(1)` — the token
   is the first literal of `ga10b_port.rs::rung5g_run`, 28 bytes, under this feature's cfg and nothing else;
   the `measured(1)` count is the seat's to take on the artifact (`LC_ALL=C grep -a -o -F`). The same gap
   stands for `ga10bprobe5b/5e/5d/5s/5m` since BANNERROWS (2026-10-06) — none has a row.

## 5. Compile record

`unaos/target/user_blob.bin` was absent in this worktree; it was supplied by copying the trunk worktree's
(`/home/user/UnaOS/unaos/target/user_blob.bin`, 51 bytes, head4 `280080d2` — the same `_start` assertion
`ensure_user_blob` makes) instead of running the 97-leg `./arroyo check`. The arc's command was run ONCE with
`-Z profile-rustflags --config 'profile.dev.package.unaos_kernel.rustflags=["--cfg","feature=\"ga10bprobe5\"",
"--cfg","feature=\"ga10bprobe4a\"","--cfg","feature=\"ga10bprobe5a\"","--cfg","feature=\"ga10bprobe5g\""]'` added,
so the ONE compile lexes `ga10b_port.rs` (the image line carries no PROBE5 feature, and `ga10bprobe5g` is not
yet a Cargo feature — item 4.1). Result: see the commit body.
