# GA10B-RUNG5-BRIEF — what the boot ROM actually wants, what our FAIL verdict can and cannot mean, and the fork

**Status: FACT-FINDING AND FROZEN FORK. No code in this arc, and none proposed for the next one until
Peter rules.** This document is the sequel to
[`GA10B-RUNG4-BRIEF.md`](GA10B-RUNG4-BRIEF.md), written in its form and to its discipline: every
fact carries its source and that source's licence; every inference is marked as one; every absence
names the population it was looked for in. It is a Group B document — see §0.

**Headline.** Rung 4 passed twice (ledger [`orin-ledger.md`](../orin-ledger.md) A51, A55): the GSP
boot ROM ran under our direction and returned `br_retcode=0x00000002` — `RESULT = FAIL`. The
ladder recorded one wall in front of rung 5. Reading the public sources against our own wire, there
are **two**, and they are not the same kind of thing:

1. **An ENCODING wall that may be ours.** NVIDIA's own published, MIT-licensed Hopper bootstrap
   writes the three BCR DMA addresses **right-shifted by 8** — the register holds the physical
   address in 256-byte units. Rung 4b wrote them **unshifted** (§2.1). If GA10B's boot ROM shares
   that encoding, the ROM was pointed at `0x80_2000_0000`, roughly 512 GiB up, and never looked at
   our buffer at all. The flight cannot tell the two readings apart, and §2.5 says why.
2. **The SIGNATURE wall, which no layout fix moves.** The manifest at `pkcparam` is
   public-key-crypto parameters for an image signed with NVIDIA's key. A perfectly laid-out,
   correctly-shifted, correctly-sized **unsigned** image returns the same `0x2` we already have.

**So the honest position is: `br_retcode` is not an oracle for rung 5, and never was.** It has one
bit of information in it and we have already spent it. §2.6 names the two oracles that do
discriminate — both already instrumented, both already flown, both already reading the negative
side.

And the fork, in one line: **no public source supports a blob-free path to any GPU engine on GA10B**
(§3), and the L4T firmware that would unblock the blob path is **not in `linux-firmware`** — it
ships only under NVIDIA's own agreement (§4). That is Peter's ruling to make, not this seat's.

---

## 0. Provenance, clean-room posture and licence posture (read first)

**Group boundary.** The author of this brief **read no `nvgpu`** and performed no
[`CLEAN_ROOM_POLICY.md`](../../../MANIFESTO/CLEAN_ROOM_POLICY.md) §6 extraction. This is a Group B
document. It imposes no new group constraint on any implementing seat.

**Where §1's facts come from, and under what licence** (LAWS §3: GPL-3.0-or-later; GPLv2-only code
is never copied in; **hardware facts are always usable**; proprietary blobs never):

| source | licence | how it is used here |
|---|---|---|
| NVIDIA `open-gpu-kernel-modules`, `src/common/inc/swref/published/hopper/gh100/dev_riscv_pri.h` | **MIT** (SPDX header read in the file) | register offsets, field bit ranges, enumerated values |
| NVIDIA `open-gpu-kernel-modules`, `src/common/inc/swref/published/ampere/ga102/dev_riscv_pri.h` | **MIT** (SPDX header read in the file) | `BCR_CTRL` and `CPUCTL` field decomposition — the register that decodes our `0x110` |
| NVIDIA `open-gpu-kernel-modules`, `src/nvidia/src/kernel/gpu/gsp/arch/hopper/kernel_gsp_gh100.c` | **MIT** (SPDX header read in the file) | the programming ORDER, the address ENCODING, the boot-params channel, the error channel |
| Linux `drm/nouveau` devfreq posting, Aaron Kling, 2025-08-31 (nouveau list) | mailing-list prose; the patch's new files carry **MIT** | one fact about which Tegra chips nouveau's register map covers |
| Linux `gpu/nova-core` Turing series, Timur Tabi, 2026-01-22 (nouveau list) | mailing-list prose about **GPL-2.0** kernel code | facts about what firmware a driver needs on Turing and later; **no code read into this tree** |
| Arch Linux `linux-firmware-nvidia` package file list, read 2026-09-12 | package index (a listing, not a work) | the enumeration behind §3's and §4's absence claim |
| NVIDIA Jetson Linux r36.4.4 Package Manifest (`docs.nvidia.com`) | NVIDIA documentation | that `lib/firmware/nvidia/ga10b/*` exists in the BSP and which agreement governs it |
| NVIDIA Driver License Agreement, v. 23 November 2023 (`docs.nvidia.com/jetson/jetpack/eula/`) | the agreement itself | §4's licence facts |

**No code, macro, struct, comment or transliteration from any of those sources appears in this
document.** Offsets, bit positions, enumerated values, orderings and structure sizes are hardware
and interface facts, which LAWS §3 makes usable regardless of the source's licence. Verbatim
quotation is held to **one short fragment, in §4**, by this tree's copyright discipline; the
authoritative licence text is the file that ships in the BSP and Peter reads it there, not here.

**Status of §1's facts inside this tree: PUBLIC-READ, not ACKED.** The ACKED facts file
([`ga10b-probe-rung1.facts.md`](../../../../unaos/docs/dev/OS/09_PLATFORM/ga10b-facts/ga10b-probe-rung1.facts.md))
remains the only source that may gate code. Everything in §1 is read from a URL by one seat on one
day; it is **corroboration and design input**, and the ladder's rule stands — a public fact must be
pointer-verified by a Group A pass before any rung imports it (LADDER provenance table, "recalled …
re-verify before import"). **This brief proposes no code, so nothing here gates anything.** The two
facts that would matter most if a rung ever used them (the address shift, the boot-params channel)
are marked ⚠ where they appear.

**Reading discipline for §1 against GA10B.** GH100 is Hopper; GA102 is Ampere dGPU; GA10B is Ampere
Tegra. The register **offsets** line up exactly with what the ACKED facts file already carries for
GA10B — `cpuctl 0x388`, `br_retcode 0x65c`, `bcr_ctrl 0x668`, `bcr_dmacfg 0x66c`, the DMA address
block `0x670`–`0x684` — and our own die read `bcr_dmacfg` accepting `target = 0x2` exactly where
GH100's header puts `NONCOHERENT_SYSMEM`. That is strong corroboration of a **shared register
interface**. It is **not** proof of shared boot-ROM **behaviour**, and the shift in §1.2 is
behaviour. Marked accordingly.

---

## 1. The FMC/BCR descriptor the boot ROM expects, from public sources

### 1.1 The registers and their fields

All offsets are falcon2/priscv-base-relative (GA10B: base `0x111000`, so BAR0-absolute is
`0x17111000 + off`). Fields from the MIT headers named in §0.

| register | off | fields, bit ranges and values | source |
|---|---|---|---|
| `BCR_CTRL` | `0x668` | `VALID` bit 0 (`FALSE` 0 / `TRUE` 1) · `CORE_SELECT` bit 4 (`FALCON` 0 / `RISCV` 1) · `BRFETCH` bit 8 (`FALSE` 0 / `TRUE` 1) | ga102 `dev_riscv_pri.h`, MIT |
| `BCR_DMACFG` | `0x66c` | `TARGET` bits 1:0 (`LOCAL_FB` / `COHERENT_SYSMEM` / `NONCOHERENT_SYSMEM` / `IO`, in that order) · `POINTER_WALKING` bit 28 · `LOCK` bit 31 (`UNLOCKED` / `LOCKED`) | gh100 `dev_riscv_pri.h`, MIT |
| `BCR_DMAADDR_PKCPARAM_LO` / `_HI` | `0x670` / `0x674` | LO bits 31:0 · **HI bits 11:0** | gh100 `dev_riscv_pri.h`, MIT |
| `BCR_DMAADDR_FMCCODE_LO` / `_HI` | `0x678` / `0x67c` | LO bits 31:0 · **HI bits 11:0** | same |
| `BCR_DMAADDR_FMCDATA_LO` / `_HI` | `0x680` / `0x684` | LO bits 31:0 · **HI bits 11:0** | same |
| `CPUCTL` | `0x388` | `STARTCPU` bit 0, **write-only** · `HALTED` bit 4, read-only · `STOPPED` bit 5, read-only · `ACTIVE_STAT` bit 7, read-only | gh100 + ga102 `dev_riscv_pri.h`, MIT |

**The HI registers are twelve bits wide, not thirty-two.** A DMA address is therefore a 44-bit
register quantity — which, with §1.2's shift, is a 52-bit physical address, and without it a 44-bit
one. Both fit; the width alone does not settle the encoding.

### 1.2 ⚠ The address encoding — the finding

`kernel_gsp_gh100.c` (MIT) defines a constant whose value is **8** and uses it as a **right-shift
count** on the physical address before splitting it into LO/HI: the register is written with
`(image physical address + section offset) >> 8`. Read twice, on two separate fetches of the file,
with agreeing answers (§7).

**Consequences, as facts about that source:**

- the register holds the address in **256-byte units**, so every one of the three buffers must be
  **256-byte aligned** for the encoding to be lossless;
- the constant is *named* for alignment and *used* as a shift; both readings give the same
  requirement, so the ambiguity in its name does not change the constraint.

⚠ **This is Hopper behaviour, read from Hopper source. Whether the GA10B boot ROM decodes the same
registers the same way is UNVERIFIED on this die, and it is the single most valuable unknown on the
ladder** — because rung 4b wrote the addresses **unshifted** (§2.1), so under this encoding the
flown ignition pointed the ROM at an address roughly 512 GiB above DRAM.

### 1.3 What the three pointers point at

In the Hopper bootstrap the three addresses are **three offsets into one contiguous image**, taken
from that image's own RISC-V ucode descriptor: a *monitor code* offset, a *monitor data* offset and
a *manifest* offset. `fmccode` is the FMC's code section; `fmcdata` is its data section; `pkcparam`
is the **manifest** — public-key-crypto parameters, i.e. the signature material the boot ROM
verifies the image against. FMC is *first mutable code*; on this class of part the FMC is the ACR,
and it is the thing that carves the WPR and boots everything else.

**Order and sizes.** The order in memory is the image's, not the programmer's: the descriptor
supplies three offsets and the BCR is given three absolute addresses derived from them. **No public
source read here states a size or a fixed layout for the manifest.** Sizes: **UNKNOWN**.

**What this says about rung 4's buffer.** Rung 4a chose `fmcdata` at +512 KiB and `pkcparam` at
+1 MiB inside its own 2 MiB window, and said plainly that those offsets were ours to choose because
nothing on the die constrained them ([`GA10B-RUNG4-BRIEF.md`](GA10B-RUNG4-BRIEF.md) §3.2). That was
correct **for rung 4**, whose payload was a fill pattern. It is **not** correct for a real image:
with a real image the three offsets are dictated by the image, and the only freedom left is where
the image as a whole is placed — which must then be 256-byte aligned if §1.2 holds.

### 1.4 ⚠ The boot-parameters buffer, and the channel it travels on

Separately from the three BCR pointers, the Hopper bootstrap allocates a **GSP-FMC boot-parameters
buffer**, asserted at compile time to be **256 bytes**, and hands its physical address to the FMC by
writing the **low 32 bits to `MAILBOX0` and the high 32 bits to `MAILBOX1`**, **unshifted**, before
the start. So:

- the boot-ROM path (`fmccode`/`fmcdata`/`pkcparam`) is **shifted**;
- the FMC-argument path (`MAILBOX0`/`MAILBOX1`) is **not**;
- and they are different buffers with different consumers.

⚠ Same caveat as §1.2: Hopper source, GA10B behaviour unverified. Recorded because it is the first
public account of what else the ROM's payload expects to find, and because our own `mailbox0` read
`0x00000000` on both passes of the flown census (§2.4).

### 1.5 How `br_retcode` is encoded — what is public and what is not

**Public, from the ACKED facts file and already in the ladder:** `br_retcode` at `0x65c`, result in
bits [1:0], `FAIL` = `0x2`, `PASS` = `0x3`, `0x0`/`0x1` = still running.

**Not public, in the sources read here:** `NV_PRISCV_RISCV_BR_RETCODE` **is not defined** in the
published MIT headers for Ampere GA102, Hopper GH100 or Blackwell GB100 — all three were fetched and
searched for it by name; each defines the BCR and CPUCTL registers around it and not that one.
So **no public field decomposition of the upper 30 bits exists in the sources this seat could
reach**, and the rung-4 brief's §7 UNKNOWN ("`br_retcode`'s upper bits may or may not carry a reason
code") stands, now with a named population rather than a shrug.

**What the same sources say about where boot errors DO surface:** the Hopper driver does not poll
`br_retcode` at all in the bootstrap path. It waits on **priv-lockdown release**, and reads
**`MAILBOX0`** for GSP-FMC error codes — effectively 8-bit, zero meaning no error. That is a channel
written by the FMC **after the boot ROM has verified and launched it**, which is exactly why it
cannot describe a boot-ROM-stage rejection (§2.4).

### 1.6 What stays UNKNOWN after this reading

- whether GA10B's boot ROM uses the §1.2 shift (the ladder's most valuable single question);
- the manifest's size, internal layout and required alignment;
- the FMC image's own descriptor format (the thing that would supply the three offsets);
- the upper-bit encoding of `br_retcode` on any part, let alone this one;
- whether `MAILBOX0`/`MAILBOX1` carry FMC arguments on GA10B as they do on GH100;
- everything rung 5 proper needs — PFIFO, PBDMA, runlist, RAMFC, USERD, CE PRI, the GPU MMU page
  table format. None of it was looked for here and none of it is in the ACKED facts file
  (LADDER §Rung 5: UNKNOWN facts, Group A pass).

---

## 2. What our own wire says against that layout

Source wires, read with `LC_ALL=C awk 'index($0,"[ga10bprobe4")'`:
[`ga10b4ab-boot1.log`](../../evidence/orin27/ga10b4ab-boot1.log) (A51, the 4a+4b flight) and
[`render13-boot2.log`](../../evidence/orin27/render13-boot2.log) (A55, 4a+4b+4c from the shutdown
path).

### 2.1 What rung 4 wrote, literally

| register | written | the buffer it was meant to name |
|---|---|---|
| `fmccode_lo` / `_hi` | `0x80200000` / `0x00000000` | window base + 0 |
| `fmcdata_lo` / `_hi` | `0x80280000` / `0x00000000` | window base + 512 KiB |
| `pkcparam_lo` / `_hi` | `0x80300000` / `0x00000000` | window base + 1 MiB |
| `bcr_dmacfg` | `0x80000002` | `LOCK` | `TARGET = NONCOHERENT_SYSMEM` — confirmed correct by §1.1 |
| `bcr_ctrl` | `0x00000111` | see §2.2 |
| `priscv_cpuctl` | `0x00000001` | `STARTCPU`, confirmed bit 0 by §1.1 |

The window was `[0x80200000, 0x80400000)`, 2 MiB, Normal-NC, filled with `0x4a10b4a5`.

**The addresses are the raw physical addresses.** Under §1.2's encoding the ROM would have read
`0x80200000 << 8` = `0x80_2000_0000` — about 512 GiB, against a machine whose NSDRAM ends at
`0x26b5f0000` (~9.7 GiB, MB2's own line on the render11 wire). Under a no-shift encoding it would
have read our fill pattern. **Both are rejections; the wire does not say which happened.**

### 2.2 `bcr_ctrl = 0x110` decodes exactly, and the rung-4 inference is confirmed

The rung-4 brief §1.2 marked an INFERENCE: bit 0 of `bcr_ctrl` is the trigger/valid bit and `0x110`
is the rest of the configuration. The GA102 MIT header decomposes the register and the arithmetic is
exact:

| value | `BRFETCH` (bit 8) | `CORE_SELECT` (bit 4) | `VALID` (bit 0) |
|---|---|---|---|
| `0x110` — what the firmware left, read on three flights | `TRUE` | `RISCV` | **`FALSE`** |
| `0x111` — the ACKED SEQ's brom_config value, which 4b wrote and held | `TRUE` | `RISCV` | **`TRUE`** |
| `0x011` — the ACKED SEQ's alternate set_bcr value | `FALSE` | `RISCV` | `TRUE` |

**The inference is confirmed against a public MIT source, and the platform firmware's leftover is
now legible:** UEFI left the BCR configured for a RISC-V core with boot-ROM fetch enabled and the
descriptor marked invalid. Rung 4b's single-bit delta was precisely "mark it valid". The alternate
`0x11` differs only in `BRFETCH`, which reads as "configure the BCR but do not have the boot ROM
fetch through it" — a second, unexplored door, named here and taken by nobody (R19: recorded, not
ruled out).

### 2.3 Why `0x2`

`RESULT = FAIL`, on **sample 1 of 16** — the verdict was already latched by the first read after the
ignition write, and 4c's 20-sample, 200 ms series found `distinct=1`, first and last both
`0x00000002`. The ROM did not deliberate. It ran, rejected, and stopped: `post-ignition priscv_cpuctl
halted=1 (raw=0x00000010)`.

The rung-4 brief §2.3 named four causes a `0x2` cannot distinguish. **There are five.** Adding
§1.2's:

| # | cause | still live after this reading? |
|---|---|---|
| 1 | the payload is unsigned | **yes** — and it is sufficient on its own |
| 2 | the payload is structurally malformed (manifest layout unknown) | yes |
| 3 | the GPU read different bytes than the CPU wrote (NSDRAM carveout encryption) | yes |
| 4 | the GPU could not read at all and the ROM timed out into FAIL | yes |
| 5 | **the address encoding was wrong, so the ROM fetched from unbacked address space** | **new — §1.2** |

Cause 5 matters more than the others because it is the only one that is **ours** and the only one a
future boot can cheaply falsify (§2.6).

### 2.4 Why the reason bits are zero

4c printed `reason_bits` = bits [31:2] of `br_retcode`, OR-ed across the whole series:
`reason_bits_or=0x00000000`, 20 samples, 200 ms. Three readings, and the wire does not choose
between them:

- the GA10B boot ROM publishes no syndrome in `br_retcode`'s upper bits;
- it publishes one only for some failure classes, and not this one;
- it publishes one elsewhere — and §1.5 says where the *next* stage's errors go (`MAILBOX0`).

**`MAILBOX0` was read in both passes of the flown census and read `0x00000000`, `diff=same`.** That
is consistent and uninformative in exactly the way §1.5 predicts: `MAILBOX0` is written by the FMC,
which never ran, because the boot ROM rejected it. An empty error mailbox after a boot-ROM-stage
rejection is the expected reading, not a datum.

### 2.5 What a correctly laid-out but unsigned image would have returned

**`br_retcode = 0x00000002`. The same value we already have.** The boot ROM's job is to verify a
signature against a key we do not hold; a structurally perfect, correctly shifted, correctly aligned
image with our own manifest in it fails at the verification step and reports `RESULT = FAIL`. There
is no third result code between FAIL and PASS in the ACKED facts file, and the upper bits are zero
(§2.4).

**Therefore `br_retcode` cannot be rung 5's oracle,** and any design that treats a `0x2` as a
statement about signatures — or a future `0x2` as a statement about layout — is reading one bit as
if it were four. The rung-4 brief said this about rung 4's own verdict; it is worth saying again
about every rung above it.

### 2.6 The two oracles that DO discriminate — already built, already flown, already negative

Both are in 4c's post-ignition census, and on the flown wire both read the negative side:

| oracle | what a real boot would show | what the flown boot showed |
|---|---|---|
| **priv-lockdown drop** — `falcon_hwcfg2` bit 13 | drops to 0 once a verified image is running (ACKED SEQ step 8) | `post_lockdown=1`; pre and post `0x0001a733`, `diff=same` |
| **the legacy-v1 mirror becoming readable** — `gsp_falcon_cpuctl_v1` at `0x110100` | becomes readable instead of `0xbadf5620` | `v1_readable=0`; `diff=same` on both passes |

Two supporting negatives from the same census, which bound the reach of what happened:
`bcr post-ignition: intact=8/8 -> POSTBCR-INTACT` (the ROM altered nothing in its own descriptor)
and `post-ignition census: readable=21/30 … changed=1 changed_ex_bcr_retcode=0 -> MAPDIFF-SAME`
(nothing on the die moved except the verdict register and what 4b wrote).

**One caution about the third witness, and it is a correction to how the flight has been read.**
`dmabuf-scan … words_changed=0/524288 -> DMABUF-UNTOUCHED` proves the boot ROM **did not write into**
our window. It says **nothing** about whether the ROM **read** it. A read leaves no trace, so
`DMABUF-UNTOUCHED` is compatible with cause 1, cause 2, cause 4 and cause 5 alike, and must never be
cited as evidence that the DMA did not land.

**The one cheap, blob-free experiment that would settle cause 5** — named, costed, and NOT designed
here because this brief's mandate is the fork, not another rung: re-fly 4a+4b unchanged except that
the six DMA address registers are written `pa >> 8`, and score the same three oracles plus a fabric
watch. It spends one power cycle, needs no ruling, no new address class and no new fact, and it
turns §2.3's cause 5 from a live ambiguity into a measurement — whichever way it falls. If Peter
wants it, it is a rung 4e and it is one boot (§6 Q4).

---

## 3. The honest fork: is there ANY blob-free path to a GPU engine on GA10B?

The question, stated so it can be answered: with LAWS §3's blob rule standing — **proprietary blobs
never** — can this kernel make any GA10B engine do work? A copy-engine copy, a 2D blit, or display
driven through the GPU?

### 3.1 What nouveau can do on GA10x before GSP-RM is loaded

| what | the public position | source |
|---|---|---|
| Run GR (and therefore 3D or compute) | **No.** On Turing and later the driver cannot bring the engines up by itself; it loads a chain of NVIDIA-signed firmware — a bootloader into falcon IMEM, then FWSEC out of the VBIOS to carve the WPR, then a booter, then GSP-RM. Signature validation is part of the flow, not an option in it | nova-core Turing series, Timur Tabi, 2026-01-22 |
| Load FECS / GPCCS | Only from NVIDIA-supplied signed images; nouveau fetches them as firmware files | same series; and `linux-firmware` ships `nvidia/ga102/` and the rest of the GA10x dGPU set |
| Drive display | On a **dGPU** the display engine is behind the same PCI device and nouveau does modeset without GSP-RM. **Not applicable here:** GA10B has no display engine. The Orin's panel is `nvdisplay`, a separate Tegra IP, already lit by the firmware's `simple-framebuffer` at carveout `window[5]` and driven by our CPU. "Display through the GPU" is not a thing on this SoC | this tree's own measurements, [`GA10B-HISTORY.md`](../../evidence/orin14/GA10B-HISTORY.md) rows 8–14 |
| Do any of it on **GA10B specifically** | **Nouveau does not support GA10B at all.** Its Tegra register map covers GK20A, GM20B, GP10B and GV11B; a nouveau contributor states on the list that GA10B support would require the code to be rearchitected | Aaron Kling, drm/nouveau devfreq posting, 2025-08-31 |
| Obtain GA10B firmware the way a dGPU obtains its own | **Not from `linux-firmware`.** `population = the Arch Linux linux-firmware-nvidia package file list, read 2026-09-12; the nvidia firmware directories it ships are ad10x, ga100–ga107, gb10x–gb20x, gh100, gk20a, gm20x, gp10x, gp10b, gv100, tu10x and tegra124/186/194/210; hits for ga10b = 0, hits for gv11b = 0` | that listing |

### 3.2 What our own die says, and it agrees

- `opt_priv_sec_en = 1` — production secure boot (rung 1).
- `falcon_hwcfg2` bit 13 = 1, priv-lockdown **engaged**, and **it did not drop across the ignition**
  (§2.6). Nine registers read `0xbadf5620` before and after; `became_readable = 0`.
- `fuse_opt_wpr_enabled = 1` and `fuse_opt_vpr_enabled = 1`, `fuse_opt_sec_debug_en = 0` — the die
  supports write- and video-protected regions and secure debug is fused off (4c pass 1).
- What *is* live at the memory-controller layer, decoded with the ACKED facts file's own bit names:
  `mc_elpg_enable = 0x2000000c` = `hub | l2 | xbar`, all three set. `mc_enable = 0x40000000` — bit 30
  only, and the facts file names no bit 30, so that value is **UNKNOWN** and is recorded, not read.

### 3.3 The answer, plainly

**No blob-free path to a GPU engine on GA10B is supported by any source read here, and this tree has
measured nothing that suggests one exists.** The engines a rung 5 would drive — host FIFO, PBDMA,
runlist, CE, the GPU MMU — sit behind a priv-lockdown that, on the public account of this GPU
family, is released by the ACR, and the ACR is the signed FMC the boot ROM just refused. The
blob-free ceiling stated in `orin-3d.md` §3 — CPU rasterisation into the inherited scanout — is not
moved by anything on this page.

**Said as R19 requires, so a later rung can reopen it rather than inherit a verdict:** this is
*failed under these conditions* — a production-fused GA10B with priv-lockdown engaged, no vendor
image, and the public sources as they stood on 2026-09-12. **What would falsify it**, in the order a
future seat should try:

1. **the lockdown's actual reach.** Nobody has read a host/PFIFO or CE PRI register on this die. The
   ACKED facts file has no offset for any of them, so the read cannot be written yet — but "the
   engines are locked" is currently an inference from `hwcfg2` bit 13 plus the public account, not a
   measurement at those addresses. A Group A pass that yields the host aperture makes it a one-boot
   read-only question, and rung 3's announce-before-touch discipline is what makes it survivable.
2. **`BRFETCH = FALSE` (`bcr_ctrl = 0x011`).** The ACKED SEQ's second door (§2.2) has never been
   opened. What it does is unknown; that it exists is a fact.
3. **the encoding question** (§2.6) — because a cause-5 world is one where nothing has actually been
   tested yet.

---

## 4. The L4T GA10B firmware set — facts for Peter's ruling. This brief decides nothing.

### 4.1 Where it lives

| fact | status |
|---|---|
| The Jetson Linux BSP contains firmware for the Ampere iGPU at the rootfs path `lib/firmware/nvidia/ga10b/`, and for Xavier's GV11B at `lib/firmware/nvidia/gv11b/` | **measured from a public source** — NVIDIA Jetson Linux r36.4.4 Package Manifest, `docs.nvidia.com/jetson/archives/r36.4.4/DeveloperGuide/RM/PackageManifest.html`, read 2026-09-12 |
| The manifest lists that directory by wildcard and **does not enumerate the individual files, and gives no sizes** | same |
| The GPU device on our own die is the one those files serve: the Orin's GPU node is at BAR0 `0x17000000`, which Linux names `17000000.ga10b` | corroborated by NVIDIA developer-forum kernel logs, and by our own DTB walk `gpu@ node: BAR0=0x17000000` |
| Upstream `linux-firmware` carries **no** `nvidia/ga10b/` directory (enumeration in §3.1) | **measured** |
| The exact file names and sizes under `lib/firmware/nvidia/ga10b/` | **UNKNOWN to this seat.** No JetPack rootfs is mounted in this tree and no public listing of that directory was found. The enumeration is one command on a JetPack 6.x rootfs: `ls -l /lib/firmware/nvidia/ga10b/`. It is deliberately **not guessed here** — a guessed filename in a licence question is the kind of fact that gets quoted back in six weeks |

### 4.2 The licence

The BSP names `Tegra_Software_License_Agreement-Tegra-Linux.txt` as the agreement covering the
complete BSP (r36.4.4 Package Manifest). The JetPack EULA page publishes the **NVIDIA Driver License
Agreement, v. 23 November 2023**. Its provisions relevant to Peter's question, by section, as read
on 2026-09-12:

| section | what it provides |
|---|---|
| **1.1(4)** | Binary redistribution **is permitted** when the software is provided for use with an operating system distributed under an OSI-approved open-source licence, on the condition that the binary files are — in the agreement's own words — "not modified in any way", and that the agreement accompanies each recipient |
| **2.1** | Use is licensed **only in conjunction with microprocessors, SoCs and GPUs designed by NVIDIA and sold by NVIDIA** — a hardware restriction, not a distribution one |
| **2.2** | Reverse engineering, decompiling or disassembling the binary software is **prohibited** |
| **2.3** | Modifying the binary software, or creating derivative works of it, is **prohibited** |

**The one verbatim fragment above is the whole of what this document quotes;** the rest is
paraphrase, and the authoritative text is the agreement file in the BSP and the published page at
`docs.nvidia.com/jetson/jetpack/eula/`. Peter reads it there.

### 4.3 The shape of the decision, stated neutrally

The facts, laid beside each other and nothing concluded from them:

- UnaOS is GPL-3.0-or-later. GPL-3.0 is an OSI-approved open-source licence.
- Section 1.1(4) permits redistributing the unmodified binaries alongside such a system, with the
  agreement carried to each recipient.
- Section 2.1 restricts *use* to NVIDIA hardware. UnaOS runs on a Jetson AGX Orin, x86 and a Pi 4;
  the firmware would be staged only for the Orin.
- **LAWS §3 says: proprietary blobs never.** That is this tree's own rule, stricter than the
  licence, and it is the rule that actually blocks the path.
- `CLEAN_ROOM_POLICY.md` §4 (the bunker rule) is the policy Peter rules under.

So the question is not primarily whether NVIDIA permits it. **It is whether UnaOS's own rule is
amended, and by whom.** §6 Q1.

---

## 5. Rung 5 IF the blob path were ruled in

Presented as a conditional design. Nothing here is built, and nothing here is built *first*: §5.1's
preconditions come before any of it.

**And a correction to the ladder while we are here.** [`GA10B-LADDER.md`](../../evidence/orin14/GA10B-LADDER.md)
§Rung 5 asks one question — "can the host FIFO run one copy-engine job?" — that cannot be one rung,
because a blob-ruled-in world puts a whole signed-boot sequence in front of it. Rung 5 splits:

- **rung 5a — the vendor ignition.** Hand the boot ROM NVIDIA's own FMC triple and get a non-FAIL
  verdict. One boot, one question, and everything in §5.2–§5.4 is about this.
- **rung 5b — the host FIFO**, which is the ladder's old rung 5 and which cannot be designed at all
  until a Group A pass yields the host aperture (§5.5).

### 5.1 Preconditions — the rung-4 ladder re-proven, on the boot, from the wire

Rung 5a inherits rung 4's §3.1 preconditions unchanged and adds three. Nothing is inherited from a
previous flight's log; everything is re-proven this boot (LAWS §5: an inherited success has its
capture re-read before it is built on).

| # | precondition | pass condition | STOP rule |
|---|---|---|---|
| P0–P4 | rung 4a's own preconditions, verbatim: the BPMP bracket with an explicit `pg` readback, `bcr_dmacfg` lock bit 0, `bcr_ctrl` baseline printed, the rung's own NC window seated, the window filled | as rung 4 §3.1 | as rung 4 §3.1, zero writes, RETURN |
| P5 | **rung 4a re-run to `BCR-ALLHELD` this boot** — 7 of 7 held and restored | `bcrheld=7/7` | anything else → `REFUSED reason=bcr-not-allheld`, zero ignition |
| P6 | **the image is present, whole and placed** — a sha256 of each staged section computed on the boot and printed beside the size, and the image base proven 256-byte aligned | all three sections sized and digested, base alignment 0 | any mismatch → `REFUSED reason=image-*`, zero ignition. **A digest that does not match what the media staged is a media fault, not a GPU one, and the rung must say which it saw** |
| P7 | **the three BCR addresses derived from the image's own descriptor**, not chosen — each printed as `section=<name> off=0x… pa=0x… reg=0x…` so the shift is visible on the wire | three addresses derived | descriptor unreadable → `REFUSED reason=no-descriptor` |

### 5.2 The smallest ignition that returns a non-FAIL verdict

The ignition itself is **rung 4b unchanged** — same seven writes, same lock, same `bcr_ctrl = 0x111`,
same `STARTCPU`, same bounded poll, same `SYSTEM_OFF` on every path. That is the point: **rung 5a
changes the payload and nothing else.** A rung that changes two things answers neither.

The three deltas, and they are all in the payload:

1. the three buffers hold NVIDIA's FMC code, FMC data and manifest at the offsets the image's
   descriptor names, instead of a fill pattern;
2. the addresses are written in whatever encoding §2.6's experiment established — **the encoding
   question is settled BEFORE this rung flies, not inside it**;
3. ⚠ if §1.4 holds on this die, the boot-parameters buffer's physical address is written to
   `MAILBOX0`/`MAILBOX1` unshifted before `STARTCPU`. **`MAILBOX0` is also the FMC's error channel
   (§1.5), so a rung that writes an argument address there must read it back after the run and say
   which of the two meanings it is reporting** — an argument pointer and an 8-bit error code in the
   same register is precisely the "two objects, one name" shape the rung-4 bounding table row D was
   built for.

**The verdict predicate.** `PASS` is **not** `br_retcode = 0x3` alone. It is the conjunction, because
§2.5 makes the single code untrustworthy:

```
br_result == 0x3  AND  post-ignition hwcfg2 lockdown == 0  AND  post-ignition v1_readable == 1
```

All three are already implemented, already flown, and already known to read the negative side on an
unsigned payload (§2.6) — so this rung's go-red proof exists **before it flies**, which is a thing
almost no rung on this ladder has been able to say.

### 5.3 The bounding table (rung 4 §4 / §10.5 form: the sibling each witness must exclude, two wires each)

Rows A–D belong to rung 4 §4 and rows E–H to §10.5; these are I–L. Look-around free, as `foreman`
requires.

| # | new witness | the SIBLING it must exclude | the naive bound and why it FAILS | the bound that HOLDS | how each is proven to fire |
|---|---|---|---|---|---|
| **I** | `[ga10bprobe5a] verdict … -> BROM-VERDICT-PASS` (the rung's PASS) | rung 4b's own `BROM-VERDICT-PASS` arm, which exists in the vocabulary and would be a **measurement error** there (rung 4 §5 F8) — and which can appear in the same capture if a boot runs both | `index($0,"BROM-VERDICT-PASS")` — **FALSE HIT** on 4b's line, and it scores an error arm as a success arm | the family token is INSIDE the bound: `index($0,"[ga10bprobe5a] … -> BROM-VERDICT-PASS")`, and the rung prints its family on the verdict line | **sibling wire:** the flown 4a+4b capture → 0 hits for the 5a row, and the 4b scorer still exits 0. **real wire:** a 5a capture → exactly 1 |
| **J** | `lockdown=0` — the priv-lockdown DROP, the rung's second conjunct | `post_lockdown=1` on every capture the ladder has ever produced, and `lockdown=1` as a substring of nothing else | `index($0,"lockdown=0")` is sound ONLY because the field is printed as a single digit with no padding; **a two-digit field would make `lockdown=0` a prefix of `lockdown=01`** | print the conjunct as a **fixed-width, fully-spelled** arm token — `POSTLOCK-DROPPED` / `POSTLOCK-HELD`, neither a prefix or substring of the other — and bound on the arm, never on the digit | **sibling wire:** `render13-boot2.log` as it stands → `POSTLOCK-HELD`, the DROPPED row scores 0. **real wire:** a drop → 1. Both run from files before the flight |
| **K** | `MAILBOX0` read back after the run, when the rung has ALSO written an argument address into it (§5.2 delta 3) | the same register's **pre-ignition** read, and its meaning as an **argument pointer** rather than an error code | `index($0,"mailbox0")` conflates three objects in one capture: 4c's pre read, 4c's post read, and 5a's argument write | the rung prints the **role** in the token, not just the register: `mbox-arg-written=0x…`, `mbox-post-read=0x…` and an explicit `mbox-post-meaning=<arg-echo\|fmc-error\|unchanged>` decided in code from what was written | **sibling wire:** the flown 4c capture (mailbox0 read twice, no argument write) → `mbox-arg-written` scores 0. **real wire:** a 5a capture → exactly one of each. Assert **exactly one**, as row D does |
| **L** | the image-integrity accounting of P6 — `image_sections=3 image_digests_ok=3` | a capture where the media staged a short or absent section, which must **not** be scored as a GPU result | "the rung ran" would pass a boot that ignited on a truncated image and blame the GPU for a media fault | assert `image_sections == image_digests_ok == 3` **and** that the rung's last line is not an announce; a `REFUSED reason=image-*` capture scores as REFUSED, never as a verdict | **sibling wire:** truncate one section's digest line in a copy → exit 1, naming the section. **real wire:** exit 0 |

**Standing addition, inherited from rung 4 §4 and repeated because it is the one that gets skipped:**
run every row **both directions before the flight, from files, capturing the scorer's own exit
code**. If you cannot name the exit code, you do not have the claim.

### 5.4 Fail shapes, named ahead (R19: each is "failed under \<conditions\>", knob and code KEPT)

F1–F9 are rung 4 §5 and F10–F18 are §10.6; these are F19 onward.

| id | shape on the wire | reading | what rung 5a does about it |
|---|---|---|---|
| **F19** | `br_retcode = 0x00000002` again, with a real signed image, correct encoding, digests verified | the image is not the one this die's ROM will accept — wrong part, wrong key, wrong generation, or the manifest is not where the descriptor said | STOP. Record the image's identity and digests. **Do not iterate images in one session**: each attempt is one boot and one power cycle, and a session that tries four has spent four cold boots and learned one thing |
| **F20** | `br_retcode` reaches `0x3` but `POSTLOCK-HELD` and `v1_readable=0` | the verdict says PASS and no observable on the die moved. **Treat as a measurement error**, exactly as rung 4 §5 F8 treats an unexplained PASS | STOP and report; re-fly from a cold boot before anything is built on it. The conjunction in §5.2 exists for this shape |
| **F21** | fabric RAS after the ignition — SNOC/ACI record, `=== AARCH64 EXCEPTION`, BL31 unhandled EL3, or a spontaneous reboot | rung 4 §5 **F2 arriving for real**: with a real image the ROM's fetch extent is no longer bounded by anything we control, and a signed image's own DMA reach is not ours to limit | STOP; this is the shape that can need a manual power cut. Record the RAS `ADDR` against the §2.2 window map of the rung-4 brief. This is the risk that makes Q2 a question and not a footnote |
| **F22** | `REFUSED reason=image-digest-mismatch` | the media staged something other than what was built | a media fault. Do not touch the GPU. The rung's value here is that it says *media*, and does not spend a power cycle proving it |
| **F23** | `mbox-post-meaning=fmc-error` with a non-zero 8-bit code | **the best failure on this list**: the boot ROM verified and LAUNCHED the image, and the FMC then failed. Everything about the ROM's wall is behind us and the error is in the next layer | a datum of the first order. Record the code. The lockdown state decides whether anything else is now reachable |
| **F24** | `mbox-post-meaning=arg-echo` — the argument address read back unchanged | nothing consumed the argument; consistent with the ROM never launching the FMC | fold with the verdict; it is corroboration, never a verdict of its own |
| **F25** | the encoding question was never settled and the rung flew anyway | the flight cannot attribute its own result | **this is a process failure, not a hardware one.** §5.2 delta 2 exists to make it impossible; if it happens, the boot is not scored |
| **F26** | the boot never reaches the rung — no `[ga10bprobe5a]` line at all | knob did not arm, or the feature did not reach the artifact | LAWS §5, full-knob gate: `⚡ kernel features:` must carry the feature and `LC_ALL=C grep -a -o -F` must find the witness family in the flight artifact. A compiled feature is not a reachable one |

### 5.5 What rung 5a does NOT attempt

- **It does not run a copy, a blit or a triangle.** It gets a verdict. The ladder's old rung 5
  question (host FIFO, PBDMA, runlist, RAMFC, USERD, CE PRI, GPU MMU) becomes **rung 5b** and is not
  designable today: the ACKED facts file carries **no offset for any of those blocks**, and the
  LADDER already records that as a Group A pass.
- **It does not put a pixel on the panel through the GPU.** Unchanged from rung 4 §6, and §3.1's row
  on display makes it structural rather than a matter of sequencing: this SoC's display is not in
  the GPU.
- **It does not read the WPR, the MC GSC or any carveout-config register.** XCARVE-8's rejection
  stands and rung 5a inherits it (rung 4 §2.2).
- **It does not touch a display register, issue `MRQ_STRAP`, or ignite the PMU.** Unchanged.
- **It does not make the blob ruling, stage a blob, name one as a file, fetch one, or embed one.**
  Nothing in §5 is built before §6 Q1 is answered.
- **It does not claim the board is left as found.** It spends the power cycle by design and ends in
  `SYSTEM_OFF`.

---

## 6. Questions only Peter can answer

These are asked, not guessed. Nothing in §5 is implemented before they are answered.

1. **The blob rule itself.** LAWS §3 says *proprietary blobs never*. NVIDIA's agreement (§4.2)
   appears to permit redistributing the unmodified binaries alongside an OSI-licensed system, and
   restricts *use* to NVIDIA hardware — which is where the firmware would run. **The licence is not
   the blocker; our own rule is.** Do you amend the rule for this one case — a vendor firmware image
   staged on the Orin's media, never linked, never modified, carried with its agreement — or does
   *never* mean never, and the GA10B ladder stops at rung 4 permanently?

2. **If it is ruled in: is rung 5a flown, and attended?** It spends a power cycle by design, and fail
   shape **F21** — a fabric RAS from a real image whose DMA reach we do not bound — is a harder
   version of the risk that has taken this box down most often. Attended at the bench, with a manual
   power cut acceptable?

3. **What is enumerated, and by whom?** §4.1 leaves the file names and sizes UNKNOWN on purpose. The
   enumeration is `ls -l /lib/firmware/nvidia/ga10b/` on a JetPack 6.x rootfs, plus reading the
   agreement file that ships beside it. Do you want a seat to do that on the bench Orin, or do you
   want nothing from that rootfs read until the ruling in Q1 is made?

4. **The cheap experiment, which needs no ruling at all.** §2.6: re-fly rung 4 unchanged except that
   the six BCR DMA addresses are written `pa >> 8`. One boot, one power cycle, no new fact, no new
   address class, no blob — and it converts §2.3's cause 5 from a live ambiguity into a measurement
   whichever way it falls. It is the only rung on this page that is free of Q1. **Fly it next?**

5. **The second door.** `bcr_ctrl = 0x011` — `BRFETCH = FALSE` — is in the ACKED SEQ and has never
   been tried (§2.2). It is a datum nobody has spent a boot on, and this seat has no theory about
   what it does. Worth a boot, or noise?

---

## 7. Verification posture of this document

Per rung 4 §9 and orin 22 BULLETIN §14 as sharpened by pi 9: a gate is skipped only when the change
cannot affect **what the gate asserts**, and the assertion must be **named**.

**What this arc changes.** One new Markdown file, a one-line pointer appended to another Markdown
file, and one ledger row. **No `.rs` file is touched.** The kernel batteries assert properties of
compiled kernel images — `arroyo check`'s per-leg type-check, the QEMU specs' runtime witnesses,
`kernel8.img` byte identity. **No assertion any of them makes can be affected by a file the build
does not read.** This is the "no `.rs` file touched" safe form, not a bare "n/a", and specifically
not the rmbp B94 case (a comment inside a `.rs` file moves `panic::Location` line numbers and
therefore image bytes; none of these three files is a `.rs`).

**What ran.** `bash unaos/scripts/ledger-check.sh` — the gate the brief names, and the only one this
change can affect, because it is the only gate that reads these files. Its exit code is quoted in
the commit and in the report. **No `arroyo` verb was run and none is owed**: this arc compiles
nothing.

**What was measured.**

- The two flown wires were re-read in this tree with `LC_ALL=C awk 'index($0,"[ga10bprobe4")'` and
  `index($0,"[ga10bprobe4c]")` — control bytes in these logs break bare `grep` (LAWS §5). Every
  value quoted in §2 is from those captures, not from the ledger's prose about them.
- Every register offset and field in §1.1 was read from the named file by URL on 2026-09-12, and
  each row names the file it came from and that file's SPDX licence, read in the same fetch.
- §1.2's shift was read **twice, in two separate fetches with different prompts**, and both reads
  agreed on the constant's value and on its use as a right-shift. It is marked ⚠ anyway, because
  two readings of one Hopper file are not a GA10B measurement.
- §3.1's and §4.1's absence claim names its population: `the Arch Linux linux-firmware-nvidia
  package file list, read 2026-09-12`, in which `ga10b` and `gv11b` each return zero hits while
  `gp10b` and four tegra directories return files.

**What was NOT measured, stated so no reader can lose it.**

- **Nothing was run on hardware.** No boot, no flight, no register was touched by this arc.
- `NV_PRISCV_RISCV_BR_RETCODE` was **not found** in the three published MIT headers searched for it
  by name (GA102, GH100, GB100). That is "not found in that scope", not "does not exist".
- The GA10B firmware file names and sizes were **not enumerated** (§4.1). No JetPack rootfs was read.
- Whether GA10B's boot ROM shares Hopper's address encoding was **not measured** and is §6 Q4.
- No source in this document has been pointer-verified by a Group A pass, and none of it may gate
  code until one does. **This brief gates nothing: it proposes no code.**
