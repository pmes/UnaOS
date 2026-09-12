# GA10B-RUNG5-BRIEF — what the boot ROM actually wants, what our FAIL verdict can and cannot mean, and the fork

**Status: THE FORK IS RULED, AND §5 IS A REAL DESIGN.** R52 (Peter, 2026-09-12) amended LAWS §3 for
ONE case — a vendor firmware image staged unmodified on the board's media, loaded as data, never
linked, never committed — so §4.1's file set is now **enumerated from public manifests** and §5 is a
design rather than a conditional. **Still no code in this arc, and rung 5a does not fly before §6's
remaining questions are answered and the encoding experiment (§2.6) has flown.** This document is
the sequel to
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
ships only under NVIDIA's own agreement (§4). That was Peter's ruling to make, and he made it.

**What R52 changed, and what it did not.** The blob path is open for ONE thing: staging NVIDIA's own
unmodified firmware files on the Orin's boot media beside the agreement they ship under, and letting
the kernel read them from that volume as data. It is not open for linking, embedding, committing,
repackaging, decrypting or modifying any of them, and it does not by itself buy a flight: **rung 5a
still sits behind the encoding experiment** (§2.6 — the free one, which needs no ruling and which
this design now takes as a precondition), behind Peter's attend-and-power-cut answer (§6 Q2), and
behind a download Peter alone runs (§4.1.3, §6 Q6). The enumeration in §4.1 below is done:
**17 files, named, with the three the boot ROM actually wants identified** — from public package
manifests, with no JetPack rootfs mounted and no firmware fetched by this seat.

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
| NVIDIA Jetson apt index, `repo.download.nvidia.com/jetson/t234/dists/r36.4/main/binary-arm64/Packages`, read 2026-09-12 | a package index (a listing, not a work) | §4.1.3's carrier: the four r36.4.x `nvidia-l4t-firmware` versions with their sizes, SHA256 and MD5 |
| Ubuntu `linux-firmware-nvidia-tegra` **36.4.3-20250107174145-0ubuntu1**, arm64 file list on `packages.ubuntu.com/questing/arm64/linux-firmware-nvidia-tegra/filelist`, read 2026-09-12 | a package file list (a listing, not a work) | **§4.1's enumeration** — the 17 names under `nvidia/ga10b/`. The upstream version string is NVIDIA's own, character for character |
| The same package's `copyright` file on `changelogs.ubuntu.com`, read 2026-09-12 | the agreement itself, republished by the distributor | §4.2's licence clauses, read in full rather than from a summary page |
| NVIDIA Jetson Linux r36.4.4 `release_sha_hashes.txt` on `developer.download.nvidia.com`, read 2026-09-12 | a checksum manifest | §4.1.3's BSP-tarball route: published SHA-1 for `Jetson_Linux_R36.4.4_aarch64.tbz2` and for the agreement file |
| OE4T `meta-tegra`, `recipes-bsp/tegra-binaries/tegra-firmware_39.2.1.bb`, read 2026-09-12 | **MIT** (layer licence) | corroboration that the deb named `nvidia-l4t-firmware` is the carrier of `firmware/nvidia/ga10b` |
| A published listing of the Xavier (GV11B) nvgpu firmware set, `threedots.ovh` blog, 2022-05-12, read 2026-09-12 | blog prose quoting a directory listing | ONE corroborating fact in §4.1.1: the `NET?_img` name shape is a family that predates GA10B |

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

⚠ **Revised by §4.1.2, and the revision matters.** The paragraph above describes *Hopper*, where the
three sections live in one image behind one descriptor. On the r36.4.3 Tegra rootfs the same three
roles arrive as **three separate files**, so there is no descriptor to parse and the three
placements are ours to choose — subject to §1.2's 256-byte alignment. Read §4.1.2 before §5.

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
- the FMC image's own descriptor format — **moot on this rootfs** (§4.1.2: the three sections are
  three files, so nothing has to supply three offsets), but still unknown as a format;
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

## 4. The L4T GA10B firmware set — the facts Peter ruled on, and the enumeration R52 unblocked

### 4.1 Where it lives

| fact | status |
|---|---|
| The Jetson Linux BSP contains firmware for the Ampere iGPU at the rootfs path `lib/firmware/nvidia/ga10b/`, and for Xavier's GV11B at `lib/firmware/nvidia/gv11b/` | **measured from a public source** — NVIDIA Jetson Linux r36.4.4 Package Manifest, `docs.nvidia.com/jetson/archives/r36.4.4/DeveloperGuide/RM/PackageManifest.html`, read 2026-09-12 |
| The manifest lists that directory by wildcard and **does not enumerate the individual files, and gives no sizes** | same |
| The GPU device on our own die is the one those files serve: the Orin's GPU node is at BAR0 `0x17000000`, which Linux names `17000000.ga10b` | corroborated by NVIDIA developer-forum kernel logs, and by our own DTB walk `gpu@ node: BAR0=0x17000000` |
| Upstream `linux-firmware` carries **no** `nvidia/ga10b/` directory (enumeration in §3.1) | **measured** |
| The exact file **names** under `lib/firmware/nvidia/ga10b/` for r36.4.3 | **ENUMERATED — §4.1.1 below.** Not from a rootfs and not guessed: from the arm64 file list of a public package whose upstream version string is NVIDIA's own (`36.4.3-20250107174145`) |
| The exact file **sizes** | **STILL UNKNOWN, and they stay UNKNOWN here.** No public listing read gives a byte count for any of the 17 files; `population = the sources in §0 plus a targeted search for the two longest file names, hits = 0`. Sizes arrive with the download (§4.1.3) as `ls -l` output, and §5.1's P8 makes the rung compute them on the boot rather than trust a number typed in a brief |

#### 4.1.1 The seventeen files, named

`population = the arm64 file list of Ubuntu package linux-firmware-nvidia-tegra
36.4.3-20250107174145-0ubuntu1, read 2026-09-12; entries under nvidia/ga10b/ = 17`. It installs them
under `/usr/lib/firmware/nvidia/ga10b/`; the BSP's own manifest path is `lib/firmware/nvidia/ga10b/`
(§4.1), and the two are the same directory under Ubuntu's `/usr` merge. **Sizes are not in that
listing and are not guessed.**

| # | file | role, and how the role is known |
|---|---|---|
| 1 | `acr-gsp.text.encrypt.bin.prod` | **the FMC code section** — INFERENCE (§4.1.2) |
| 2 | `acr-gsp.data.encrypt.bin.prod` | **the FMC data section** — INFERENCE (§4.1.2) |
| 3 | `acr-gsp.manifest.encrypt.bin.out.bin.prod` | **the manifest, i.e. the PKC parameters** — INFERENCE (§4.1.2) |
| 4 | `safety-scheduler.text.encrypt.bin.prod` | a SECOND RISC-V image of the same three-part shape; role UNKNOWN beyond its name |
| 5 | `safety-scheduler.data.encrypt.bin.prod` | same |
| 6 | `safety-scheduler.manifest.encrypt.bin.out.bin.prod` | same |
| 7 | `gpmu_ucode_next_prod_image.bin` | the PMU falcon image — rung 5a does **not** touch the PMU (§5.5) |
| 8 | `gpmu_ucode_next_prod_desc.bin` | its descriptor |
| 9 | `pmu_pkc_prod_sig.bin` | the PMU image's PKC signature |
| 10 | `fecs_encrypt_prod.bin` | the GR front-end context-switch falcon image — **rung 6**, not 5a |
| 11 | `fecs_pkc_sig_encrypt.bin` | its PKC signature |
| 12 | `gpccs_encrypt_prod.bin` | the GPC context-switch falcon image — rung 6 |
| 13 | `gpccs_pkc_sig_encrypt.bin` | its PKC signature |
| 14 | `NETA_img_prod_encrypted.bin` | **UNKNOWN.** Four `NET?_img_prod_encrypted.bin` images, and no source read here says what they are. A published listing of the Xavier (GV11B) nvgpu firmware set carries the same `NET?_img` shape, which is corroboration that the four are one family and not four roles — and nothing more than that |
| 15 | `NETB_img_prod_encrypted.bin` | UNKNOWN, as above |
| 16 | `NETC_img_prod_encrypted.bin` | UNKNOWN, as above |
| 17 | `NETD_img_prod_encrypted.bin` | UNKNOWN, as above |

**Caveat on the population, stated because it bounds every row above.** The Ubuntu package is a
repack: its own `copyright` file says it carries data extracted from NVIDIA's published debs, with
files inapplicable to current hardware dropped. So the 17 names are names that **are** in NVIDIA's
r36.4.3 set; whether NVIDIA's own deb carries a file under `ga10b/` that Ubuntu dropped is
**UNVERIFIED**, and the check is one command after the download: `dpkg-deb -c` on the NVIDIA deb
(§4.1.3), diffed against this table. Rung 5a does not need the full set — it needs three files, and a
file it does not name it does not read.

#### 4.1.2 The three the boot ROM wants — the mapping, marked as the inference it is

⚠ **INFERENCE, and the single most consequential one in this document.** Rung 4's registers want
`fmccode`, `fmcdata` and `pkcparam` (§1.1). §1.3 read, from the MIT Hopper bootstrap, that those
three are *monitor code*, *monitor data* and *manifest*. The r36.4.3 `ga10b` directory contains
exactly one triple whose names are `…text…`, `…data…` and `…manifest…` under one stem — `acr-gsp` —
and the LADDER already predicted that stem from the roles alone
([`GA10B-LADDER.md`](../../evidence/orin14/GA10B-LADDER.md), "the blobs, named, not fetched"): the ACR
is the first signed image, and on this class of part the ACR **is** the FMC (§1.3). The mapping is
therefore:

```
fmccode  <- acr-gsp.text.encrypt.bin.prod
fmcdata  <- acr-gsp.data.encrypt.bin.prod
pkcparam <- acr-gsp.manifest.encrypt.bin.out.bin.prod
```

**What is an inference and what is not.** That the three files exist and are named that way is
measured. That `text`/`data`/`manifest` map onto `fmccode`/`fmcdata`/`pkcparam` in that order is an
inference from the names plus §1.3's roles. Nothing on this die has confirmed it, and rung 5a is
built so that a wrong mapping is **cheap and legible**: it is three register values, the wire prints
which file went to which register with its digest, and a wrong assignment returns the same `0x2`
we already have (§2.5) rather than doing anything to the board.

**Three structural facts that fall out of the enumeration, and they change §5.**

1. **The triple is THREE FILES, not three offsets into one image.** Hopper's bootstrap takes the
   three offsets from one image's own RISC-V ucode descriptor (§1.3). Here the sections arrive
   separately, so **there is no descriptor to read** — which deletes rung 5a's "read the descriptor"
   precondition and replaces it with "place three files" (§5.1 P7). The freedom §1.3 said a real
   image takes away is partly given back: we choose the three placements, and each must be 256-byte
   aligned if §1.2's encoding holds.
2. **Every section is encrypted as well as signed** — `…encrypt…` / `…encrypted…` is in the name of
   all three, of the second triple, and of the FECS/GPCCS pair. INFERENCE from the names, and it
   agrees with `orin-3d.md` §3's "AES-encrypted and PKC-signed" reading already in this tree. The
   consequence for us is practical, not theoretical: **the bytes are opaque, they stay opaque, and
   nothing in this ladder inspects, decrypts or transforms them** — see §5.5 and licence clauses 2.2
   and 2.6 (§4.2).
3. **There is a second door of the same shape.** `safety-scheduler.{text,data,manifest}` is a second
   FMC-shaped triple. R19 form: named, not tried, not ruled out — §6 Q7.

#### 4.1.3 Where it can be got, and the one line Peter would run

Three public carriers were found. Two are NVIDIA's own publications and the third is a distributor's
repack of one of them; all carry the same agreement, and the choice between them is a matter of size,
of provenance, and of what else comes down with it.

| # | carrier | what it is | size | published digest |
|---|---|---|---|---|
| **A** | `https://repo.download.nvidia.com/jetson/t234/pool/main/n/nvidia-l4t-firmware/nvidia-l4t-firmware_36.4.3-20250107174145_arm64.deb` | NVIDIA's own apt pool, the deb that carries `lib/firmware/nvidia/ga10b/*` **and** the agreement file beside it | **2,295,852 bytes** (confirmed by the index and by an HTTP HEAD; no body fetched) | SHA256 `ec89d6a5c449bc6e7e89127bc7390bea7a113a69f92a4dc15462e9e943fad3ab`, MD5 `9a41f93bba148f7b92b8c2b924f43e98`, both from `dists/r36.4/main/binary-arm64/Packages` |
| **B** | `https://developer.download.nvidia.com/embedded/L4T/r36_Release_v4.4/release/Jetson_Linux_R36.4.4_aarch64.tbz2` | the whole BSP; the firmware is two tarballs deep, in `Linux_for_Tegra/nv_tegra/nvidia_drivers.tbz2` | **746,324,969 bytes** (HTTP HEAD) | SHA-1 `1039c377717e443cbabd9a1a719162dd84ab4678`, from that release's `release_sha_hashes.txt` |
| **C** | Ubuntu multiverse, `linux-firmware-nvidia-tegra_36.4.3-20250107174145-0ubuntu1_arm64.deb` | a distributor's repack of A, stripped (§4.1.1) | 1,240,622 bytes (HTTP HEAD) | published in Ubuntu's archive indices; **not** NVIDIA's own publication |

**The one line, and why it is that one.** Carrier A: it is NVIDIA's own publication, it is 2.2 MB
instead of 746 MB, it carries the agreement in the same archive as the firmware (so clause 1.1(d)'s
condition that the agreement reach each recipient is satisfiable from one download), and its digest
is published by NVIDIA in a plain-text index anyone can re-read.

```
curl -fL -O https://repo.download.nvidia.com/jetson/t234/pool/main/n/nvidia-l4t-firmware/nvidia-l4t-firmware_36.4.3-20250107174145_arm64.deb
sha256sum nvidia-l4t-firmware_36.4.3-20250107174145_arm64.deb   # expect ec89d6a5…fad3ab, 2,295,852 bytes
dpkg-deb -c  nvidia-l4t-firmware_36.4.3-20250107174145_arm64.deb | grep ga10b     # the sizes, and the §4.1.1 diff
dpkg-deb -x  nvidia-l4t-firmware_36.4.3-20250107174145_arm64.deb  fw/            # extract, unmodified
```

**Why 36.4.3 and not 36.4.4 or 36.4.7.** All four r36.4.x versions are published side by side in the
same index, with these digests:

| version | size | SHA256 |
|---|---|---|
| `36.4.0-20240912212859` | 2,295,736 | `aa81e56c2df9cb71d47fbfda0979da541d64d2f86a6e6e57043ec99ad03f3419` |
| `36.4.3-20250107174145` | 2,295,852 | `ec89d6a5c449bc6e7e89127bc7390bea7a113a69f92a4dc15462e9e943fad3ab` |
| `36.4.4-20250616085344` | 2,295,844 | `08407971957c6e454d0e53d90cf1a15cda85a59aa9da00fb3cca9c4759cd2b17` |
| `36.4.7-20250918154033` | 2,295,804 | `38dc5a85b4519a6ba1ff2792ae1e00baabbf12e1b7438b9aefdce424da154acb` |

36.4.3 is the version this tree's own ACKED GA10B facts were extracted from
([`GA10B-HISTORY.md`](../../evidence/orin14/GA10B-HISTORY.md) row 15), so it is the one whose
register interface our facts file actually describes, and it is the version whose file names are
enumerated above. **Whether the boot ROM cares about the firmware's version at all is UNKNOWN** — a
signature is a signature — so this is a consistency choice, not a measured requirement, and §6 Q6
puts it to Peter along with the download itself.

**This seat downloaded none of it.** Every number above came from a text index, a file list, a
checksum manifest or an HTTP HEAD; no firmware, tarball or package was fetched.

### 4.2 The licence

The BSP names `Tegra_Software_License_Agreement-Tegra-Linux.txt` as the agreement covering the
complete BSP (r36.4.4 Package Manifest). The JetPack EULA page publishes the **NVIDIA Driver License
Agreement, v. 23 November 2023**. Its provisions relevant to Peter's question, by section, as read
on 2026-09-12:

**Correction to the citation, made on a full reading rather than a summary.** The redistribution
clause is lettered, not numbered: it is **1.1(d)**, the fourth grant in section 1.1. The earlier
reading called it "1.1(4)", which is the same clause and the wrong name for it. The full text was
read on 2026-09-12 in the `copyright` file that the r36.4.3 firmware package itself ships
(`usr/share/doc/nvidia-l4t-firmware/copyright`; republished at
`changelogs.ubuntu.com/changelogs/pool/multiverse/l/linux-firmware-nvidia-tegra/linux-firmware-nvidia-tegra_36.4.3-20250107174145-0ubuntu1/copyright`).

| section | what it provides |
|---|---|
| **1.1(d)** | Binary redistribution **is permitted** for software provided for use with operating systems distributed under an OSI-approved open-source licence, on two conditions: that the binary files are — in the agreement's own words — "not modified in any way" (decompression excepted), and that the agreement is provided to each recipient of the software |
| **2.1** | Use is licensed **only in conjunction with microprocessors, SoCs and GPUs designed by NVIDIA and sold by NVIDIA**. The clause has two further sentences the earlier reading missed, and both bear on this ladder: firmware may be used **only in** such platforms, and the licensee may not **translate** firmware out of the architecture or language NVIDIA provides it in |
| **2.2** | Reverse engineering, decompiling or disassembling the binary software is **prohibited** |
| **2.3** | Modifying the binary software, or creating derivative works of it, is **prohibited** |
| **2.6** | Bypassing, disabling or circumventing any technical limitation, **encryption**, security, DRM or **authentication mechanism** in the software is **prohibited**. Named here because rung 5a's whole subject is an authentication mechanism — see the note below |
| **2.9** | Using the software in a manner that would cause it to become subject to an open-source licence is **prohibited**. Named here because it is the clause that makes *staged beside, never linked into* the only shape this can take in a GPL-3.0 tree |

**On clause 2.6, stated plainly because it would be dishonest to leave it implied.** Rung 5a does
not bypass, disable or circumvent the boot ROM's authentication: it hands the ROM NVIDIA's own
signed, encrypted image and lets the ROM verify it exactly as the vendor stack does. Nothing in this
ladder decrypts a section, inspects one, patches one, or attempts a signature. The rung's PASS is
"the ROM accepted a genuine image"; a design whose PASS were "the ROM accepted something we made"
would be a different document and would not be written in this tree. §5.5 carries this as a
not-attempted item so it cannot drift.

**The one verbatim fragment above is the whole of what this document quotes;** the rest is
paraphrase, and the authoritative text is the agreement file that ships with the firmware. Peter
reads it there, or at `docs.nvidia.com/jetson/jetpack/eula/`.

### 4.3 The shape of the decision — RULED

The facts, as they were laid out for the ruling:

- UnaOS is GPL-3.0-or-later. GPL-3.0 is an OSI-approved open-source licence.
- Section 1.1(d) permits redistributing the unmodified binaries alongside such a system, with the
  agreement carried to each recipient.
- Section 2.1 restricts *use* to NVIDIA hardware. UnaOS runs on a Jetson AGX Orin, x86 and a Pi 4;
  the firmware would be staged only for the Orin.
- **LAWS §3 said: proprietary blobs never.** That was this tree's own rule, stricter than the
  licence, and it was the rule that actually blocked the path.
- `CLEAN_ROOM_POLICY.md` §4 (the bunker rule) is the policy Peter ruled under.

**The ruling (R52, Peter, 2026-09-12).** LAWS §3 gains one carve-out: the GA10B firmware image may be
staged UNMODIFIED on the board's boot media beside the vendor's licence agreement and loaded by the
kernel as data from that volume. It is never linked into any UnaOS artifact, never embedded, never
committed to the repository, and never modified; **the flight's ledger row names each file, its
size, its sha256 and the licence it ships under.** That last clause is a reporting obligation on
rung 5a, and §5.7 is how it is met.

The obligations R52 puts on this design, each mapped to the thing that enforces it:

| R52 obligation | what enforces it in rung 5a |
|---|---|
| staged unmodified | §5.7's MANIFEST row is a sha256 of the file as extracted; §5.1 P6 re-computes it on the boot and REFUSES on a mismatch |
| beside the vendor's agreement | `GA10B/LICENCE.txt` is a staged file like any other and is listed in the MANIFEST (§5.7) |
| loaded as data from that volume | the VFS read path, into DRAM the rung already owns; no linker section, no `include_bytes!`, no build step (§5.2) |
| never committed to the repository | the files live only in the staged flash directory under `~/unaos-bench/flash/orin/<round>/`; **nothing under `unaos/` gains a binary**, which is LAWS §3's named enforcer (the `git ls-files` census) |
| the ledger row names each file, size, sha256 and licence | §5.7's row template, filled from the boot's own printed digests rather than from the staging host |

---

## 5. Rung 5a — the blob-backed ignition, designed for real

R52 ruled the path in, so this is a design and no longer a conditional. **Nothing here is built yet,
and nothing here is built *first*:** §5.1's preconditions come before any of it, and the first of
them is a rung that has not flown.

**And a correction to the ladder while we are here.** [`GA10B-LADDER.md`](../../evidence/orin14/GA10B-LADDER.md)
§Rung 5 asks one question — "can the host FIFO run one copy-engine job?" — that cannot be one rung,
because a blob-ruled-in world puts a whole signed-boot sequence in front of it. Rung 5 splits:

- **rung 5a — the vendor ignition.** Hand the boot ROM NVIDIA's own FMC triple and get a non-FAIL
  verdict. One boot, one question, and everything in §5.1–§5.7 is about this.
- **rung 5b — the host FIFO**, which is the ladder's old rung 5 and which cannot be designed at all
  until a Group A pass yields the host aperture (§5.5).

### 5.1 Preconditions — the rung-4 ladder re-proven, on the boot, from the wire

Rung 5a inherits rung 4's §3.1 preconditions unchanged and adds five. Nothing is inherited from a
previous flight's log; everything is re-proven this boot (LAWS §5: an inherited success has its
capture re-read before it is built on). **P5x is the one that is not about this boot at all** — it
is about a rung that must have flown before this one is built.

| # | precondition | pass condition | STOP rule |
|---|---|---|---|
| P0–P4 | rung 4a's own preconditions, verbatim: the BPMP bracket with an explicit `pg` readback, `bcr_dmacfg` lock bit 0, `bcr_ctrl` baseline printed, the rung's own NC window seated, the window filled | as rung 4 §3.1 | as rung 4 §3.1, zero writes, RETURN |
| **P5x** | **the encoding is settled** — rung 4e (§2.6: rung 4 re-flown with the six BCR addresses written `pa >> 8`) has flown and been scored, and the rung compiles the encoding it established as a constant, printed on the wire as `addr_encoding=<shift8\|raw> from=<evidence path>` | a scored 4e verdict exists in `docs/dev/evidence/` and the ledger row for it is `flown` | absent → **the rung is not built**. This is a design-time gate, not a runtime one: a 5a that flies before 4e cannot attribute its own result (fail shape F25) |
| P5 | **rung 4a re-run to `BCR-ALLHELD` this boot** — 7 of 7 held and restored | `bcrheld=7/7` | anything else → `REFUSED reason=bcr-not-allheld`, zero ignition |
| P6 | **the three files are present, whole and unmodified** — each read from the boot volume, sha256 computed **on the boot** and printed beside its byte size and the ledger row's expected digest | three files found, three digests equal to the ledger row's | a file missing → `REFUSED reason=image-absent name=<f>`; a digest mismatch → `REFUSED reason=image-digest-mismatch name=<f>`. Both are **zero MMIO** and both are media faults, not GPU ones, and the rung must say which it saw |
| P7 | **the three placements chosen and printed**, one per section — `section=fmccode file=<name> size=<n> off=0x… pa=0x… reg=0x…`, so the mapping, the alignment and the encoding are all legible on one line | three placements printed, each `pa & 0xff == 0` | any `pa` not 256-byte aligned → `REFUSED reason=image-unaligned`. **There is no descriptor to read (§4.1.2): the three files ARE the three sections**, so this precondition is arithmetic we do, not a structure we parse |
| P8 | **the window is big enough** — `need = align256(size_text) + align256(size_data) + align256(size_manifest)` computed on the boot and compared against the seated window, printed as `window_need=0x… window_have=0x…` | `need <= have` | `need > have` → `REFUSED reason=image-too-large`, zero ignition. **Never truncate, never overlap, never spill**: a short section is not a smaller image, it is a different one |

**Is the rung-4 window big enough? The honest answer is that nobody can say yet, and the design is
built so that the answer does not have to be guessed.** The seated window is 2 MiB at `0x80200000`
(`[0x80200000, 0x80400000)`, A51's `[ga10b4nc]` line). The three section sizes are **UNKNOWN**
(§4.1); the only public bound this seat could put on them is that the 17 `ga10b` files, together with
every other directory that deb carries, fit inside its 18,989 KiB installed size — which does not
bound three files usefully. So:

- **the requirement, stated as a formula rather than a number:** the window must be at least
  `align256(text) + align256(data) + align256(manifest)`, its base 256-byte aligned (the existing
  2 MiB-aligned seat satisfies that for free), and it must be the rung's own Normal-NC block so no
  other allocation can land inside the ROM's fetch extent;
- **2 MiB is plausible and unproven.** The expectation is that images of this class run to tens or a
  few hundred KiB rather than megabytes — but that expectation carries **no citation in this
  document**, it is not a measurement, and nothing in §5 rests on it. P8 is what the design rests on;
- **P8 turns the unknown into a printed comparison on the boot**, and the fix if it fires is a wider
  seat in `mmu_tegra` (4 MiB or 8 MiB, the next clean L2-split block), which is a one-line change in
  a different arc — not a truncated image.

### 5.2 The smallest ignition that returns a non-FAIL verdict

The ignition itself is **rung 4b unchanged** — same seven writes, same lock, same `bcr_ctrl = 0x111`,
same `STARTCPU`, same bounded poll, same `SYSTEM_OFF` on every path. That is the point: **rung 5a
changes the payload and nothing else.** A rung that changes two things answers neither.

The three deltas, and they are all in the payload:

1. the three buffers hold NVIDIA's FMC code, FMC data and manifest — the three files of §4.1.2, read
   from the boot volume — instead of a fill pattern;
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

#### 5.2.1 How the bytes get from the card into the window

The path is ordinary and that is the point: **the firmware is data on a filesystem, not a linked
object.** Three reads, in order, all before a single MMIO access:

1. **Locate the volume.** The boot volume is the one `fs/bootdisk.rs` already binds — the disk
   carrying the file whose bytes are this kernel's own `.text` window (A28's successor; nothing
   found prints one witness and leaves the table empty, two or more REFUSE rather than guess). Rung
   5a adds no volume logic; it resolves the mount and prints the resolved path it will read.
2. **Read each file** through the VFS (`fs::vfs::MountTable::read(path, offset, len)`, over
   `fs::fat`'s `read_file`) into the rung's own 2 MiB Normal-NC window at the placement P7 printed,
   in `text`, `data`, `manifest` order, each at the next 256-byte boundary. No copy anywhere else,
   no heap staging of a second copy, and the window is the same block rung 4 filled with
   `0x4a10b4a5` — so an unwritten tail is still the known pattern rather than uninitialised memory.
3. **Digest what landed, not what was read.** The sha256 of P6 is computed over the bytes **in the
   window**, after a `dsb sy`, through the same Normal-NC mapping — so a read that half-landed shows
   up as a digest mismatch rather than as a GPU mystery an hour later.

**The one thing about the staged names that can bite, named here rather than discovered on a
bench.** `acr-gsp.manifest.encrypt.bin.out.bin.prod` is 41 characters with six dots. That is a FAT
**long** name; it has no 8.3 form worth reading, and the `SRC.TGZ` / `SRC.SHA` convention in
`make-pi-img.sh` exists precisely because the kernel's read-only FAT reader was built to name 8.3
files. **Whether our FAT path resolves a long name with multiple dots is not asserted here** — it is
the first thing the implementing seat proves, from a file on a card, before any of this is worth
writing. If it does not, that is a kernel finding with two honest outcomes (teach the reader long
names, or stage under 8.3 aliases and record the alias map in the ledger row beside the real name),
and **renaming a file is not modifying it** — R52's "unmodified" is about bytes. The preference is
verbatim names, because a name that matches the vendor's is one less thing to get wrong twice.

### 5.3 The bounding table (rung 4 §4 / §10.5 form: the sibling each witness must exclude, two wires each)

Rows A–D belong to rung 4 §4 and rows E–H to §10.5; these are I–L. Look-around free, as `foreman`
requires.

| # | new witness | the SIBLING it must exclude | the naive bound and why it FAILS | the bound that HOLDS | how each is proven to fire |
|---|---|---|---|---|---|
| **I** | `[ga10bprobe5a] verdict … -> BROM-VERDICT-PASS` (the rung's PASS) | rung 4b's own `BROM-VERDICT-PASS` arm, which exists in the vocabulary and would be a **measurement error** there (rung 4 §5 F8) — and which can appear in the same capture if a boot runs both | `index($0,"BROM-VERDICT-PASS")` — **FALSE HIT** on 4b's line, and it scores an error arm as a success arm | the family token is INSIDE the bound: `index($0,"[ga10bprobe5a] … -> BROM-VERDICT-PASS")`, and the rung prints its family on the verdict line | **sibling wire:** the flown 4a+4b capture → 0 hits for the 5a row, and the 4b scorer still exits 0. **real wire:** a 5a capture → exactly 1 |
| **J** | `lockdown=0` — the priv-lockdown DROP, the rung's second conjunct | `post_lockdown=1` on every capture the ladder has ever produced, and `lockdown=1` as a substring of nothing else | `index($0,"lockdown=0")` is sound ONLY because the field is printed as a single digit with no padding; **a two-digit field would make `lockdown=0` a prefix of `lockdown=01`** | print the conjunct as a **fixed-width, fully-spelled** arm token — `POSTLOCK-DROPPED` / `POSTLOCK-HELD`, neither a prefix or substring of the other — and bound on the arm, never on the digit | **sibling wire:** `render13-boot2.log` as it stands → `POSTLOCK-HELD`, the DROPPED row scores 0. **real wire:** a drop → 1. Both run from files before the flight |
| **K** | `MAILBOX0` read back after the run, when the rung has ALSO written an argument address into it (§5.2 delta 3) | the same register's **pre-ignition** read, and its meaning as an **argument pointer** rather than an error code | `index($0,"mailbox0")` conflates three objects in one capture: 4c's pre read, 4c's post read, and 5a's argument write | the rung prints the **role** in the token, not just the register: `mbox-arg-written=0x…`, `mbox-post-read=0x…` and an explicit `mbox-post-meaning=<arg-echo\|fmc-error\|unchanged>` decided in code from what was written | **sibling wire:** the flown 4c capture (mailbox0 read twice, no argument write) → `mbox-arg-written` scores 0. **real wire:** a 5a capture → exactly one of each. Assert **exactly one**, as row D does |
| **L** | the image-integrity accounting of P6 — `image_sections=3 image_digests_ok=3` | a capture where the media staged a short or absent section, which must **not** be scored as a GPU result | "the rung ran" would pass a boot that ignited on a truncated image and blame the GPU for a media fault | assert `image_sections == image_digests_ok == 3` **and** that the rung's last line is not an announce; a `REFUSED reason=image-*` capture scores as REFUSED, never as a verdict | **sibling wire:** truncate one section's digest line in a copy → exit 1, naming the section. **real wire:** exit 0 |
| **M** | the per-file identity line — `file=<name> size=<n> sha256=<64 hex> expect=<64 hex> match=<0\|1>`, one per section | the OTHER two files' lines in the same capture, and the ledger row's own prose about the same digests | `index($0,"sha256=")` conflates the three, and a scorer that counts matches without binding them to a name cannot tell "three files verified" from "one file verified three times" | bind on the ROLE token, which the rung prints and the scorer does not infer: `index($0,"[ga10bprobe5a] file=acr-gsp.text")` and its two siblings; assert **exactly one line per role** and `match=1` on each | **sibling wire:** any rung-4 capture → 0 hits for all three roles. **real wire:** exactly 1 each, 3 in total. **red wire:** a copy with one `match=1` flipped to `match=0` → exit 1, naming the file |
| **N** | the window-fit accounting of P8 — `window_need=0x… window_have=0x…` | a capture that REFUSED for a different reason, and a capture with no fit line at all | "the rung printed a need" does not say it compared; a missing line would silently pass an "all checks green" scorer | assert the line is present, that `need <= have` as **numbers parsed by the scorer**, and that the presence of `REFUSED reason=image-too-large` and a verdict line are mutually exclusive in one capture | **sibling wire:** a hand-built capture with `need` larger than `have` and a verdict line → exit 1 (the mutual-exclusion assertion fires). **real wire:** need ≤ have, one verdict, exit 0 |

**Standing addition, inherited from rung 4 §4 and repeated because it is the one that gets skipped:**
run every row **both directions before the flight, from files, capturing the scorer's own exit
code**. If you cannot name the exit code, you do not have the claim.

### 5.4 Fail shapes, named ahead (R19: each is "failed under \<conditions\>", knob and code KEPT)

F1–F9 are rung 4 §5 and F10–F18 are §10.6; these are F19 onward.

| id | shape on the wire | reading | what rung 5a does about it |
|---|---|---|---|
| **F19** | `br_retcode = 0x00000002` again, with a real signed image, correct encoding, digests verified | the image is not the one this die's ROM will accept — wrong part, wrong key, wrong generation, or the three files are not the three sections (the mapping in §4.1.2 is an inference) | STOP. Record the image's identity and digests. **Do not iterate images in one session**: each attempt is one boot and one power cycle, and a session that tries four has spent four cold boots and learned one thing |
| **F20** | `br_retcode` reaches `0x3` but `POSTLOCK-HELD` and `v1_readable=0` | the verdict says PASS and no observable on the die moved. **Treat as a measurement error**, exactly as rung 4 §5 F8 treats an unexplained PASS | STOP and report; re-fly from a cold boot before anything is built on it. The conjunction in §5.2 exists for this shape |
| **F21** | fabric RAS after the ignition — SNOC/ACI record, `=== AARCH64 EXCEPTION`, BL31 unhandled EL3, or a spontaneous reboot | rung 4 §5 **F2 arriving for real**: with a real image the ROM's fetch extent is no longer bounded by anything we control, and a signed image's own DMA reach is not ours to limit | STOP; this is the shape that can need a manual power cut. Record the RAS `ADDR` against the §2.2 window map of the rung-4 brief. This is the risk that makes Q2 a question and not a footnote |
| **F22** | `REFUSED reason=image-digest-mismatch` | the media staged something other than what was built | a media fault. Do not touch the GPU. The rung's value here is that it says *media*, and does not spend a power cycle proving it |
| **F23** | `mbox-post-meaning=fmc-error` with a non-zero 8-bit code | **the best failure on this list**: the boot ROM verified and LAUNCHED the image, and the FMC then failed. Everything about the ROM's wall is behind us and the error is in the next layer | a datum of the first order. Record the code. The lockdown state decides whether anything else is now reachable |
| **F24** | `mbox-post-meaning=arg-echo` — the argument address read back unchanged | nothing consumed the argument; consistent with the ROM never launching the FMC | fold with the verdict; it is corroboration, never a verdict of its own |
| **F25** | the encoding question was never settled and the rung flew anyway | the flight cannot attribute its own result | **this is a process failure, not a hardware one.** §5.2 delta 2 exists to make it impossible; if it happens, the boot is not scored |
| **F26** | the boot never reaches the rung — no `[ga10bprobe5a]` line at all | knob did not arm, or the feature did not reach the artifact | LAWS §5, full-knob gate: `⚡ kernel features:` must carry the feature and `LC_ALL=C grep -a -o -F` must find the witness family in the flight artifact. A compiled feature is not a reachable one |
| **F27** | `REFUSED reason=image-absent name=<f>` | the card does not carry that file — a staging miss, or the FAT reader cannot name it (§5.2.1's long-name hazard) | **zero MMIO, and the boot RETURNS rather than spending the cycle.** The wire says which of the two it was: the rung prints the directory listing it actually got, so "not staged" and "staged but unnameable" are different lines, not one shrug |
| **F28** | `REFUSED reason=image-too-large window_need=0x… window_have=0x…` | the three sections do not fit the 2 MiB seat (P8) | zero MMIO, boot RETURNS. **A design datum of the first order and the cheapest on this page** — it is the first time anyone learns the real sizes, and it costs no power cycle. The fix is a wider seat in `mmu_tegra`, in its own arc, then re-fly |
| **F29** | `br_result=0x3` and `POSTLOCK-DROPPED` but `v1_readable=0` (or the mirror opens and lockdown holds) | a **partial** success: the two independent oracles disagree | not a PASS under §5.2's conjunction, and not F20's measurement error either. Report as its own arm; two oracles disagreeing IS the finding, and it is the next rung's input |
| **F30** | the volume the rung read is not the card that carries this kernel | the bench's two-media hazard (the fault `media-writer.sh` exists for) arriving at a rung that reads data files | P6 catches it as `image-absent` or `image-digest-mismatch`, never as a GPU result. The rung prints the volume's FAT serial beside the digests, so the report can name the card |

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
- **It does not touch a display register, issue `MRQ_STRAP`, or ignite the PMU.** The PMU image, its
  descriptor and its signature are three of the seventeen files (§4.1.1 rows 7–9) and rung 5a reads
  none of them. `pmu_falcon2_cpuctl` at `0x1710b388` stays read-only, exactly as rung 3 left it.
- **It does not load FECS or GPCCS.** Rows 10–13 of §4.1.1 are rung 6's, and rung 6 additionally
  needs a GA10B SASS assembler that no public specification describes (rung 4 §6).
- **It does not load the second triple.** `safety-scheduler.{text,data,manifest}` is named in §4.1.2
  and tried by nobody (R19: recorded, not ruled out — §6 Q7).
- **It does not touch the host FIFO, PBDMA, runlist, RAMFC, USERD, any CE or GR register, or the GPU
  MMU.** Those are rung 5b and they need a Group A pass the ACKED facts file has not had.
- **It does not decrypt, disassemble, inspect, patch, repackage, recompress or rename-in-the-bytes
  any vendor file.** The three sections are opaque input; the only operations performed on them are
  *read*, *place*, *digest*. This is R52's "unmodified" and licence clauses 2.2, 2.3 and 2.6 (§4.2),
  and it is asserted here so that a later arc cannot drift into it by convenience.
- **It does not commit a vendor file to the repository, embed one in an artifact, or link one.** The
  files exist only on the staged media (§5.7). LAWS §3's enforcer — no binary firmware under
  `unaos/` — must stay green through this rung and every rung above it.
- **It does not make the blob ruling.** R52 made it; this design implements what R52 allows and
  nothing beside it.
- **It does not claim the board is left as found.** It spends the power cycle by design and ends in
  `SYSTEM_OFF`.

### 5.6 The knob shape

The rung-3/rung-4 precedent, unchanged in shape so that an unexpected value can never buy an
ignition:

| `UNAOS_GA10B_PROBE5=` | features | what the boot does |
|---|---|---|
| `1` | `ga10bprobe5a` | the whole rung: 4a's bracket and census, P5x–P8, the three reads of `/GA10B/<file>` from the boot volume, the ignition, the post-ignition oracles, `SYSTEM_OFF` |
| anything else non-empty | none | **nothing** — the rung does not arm, the boot proceeds. Rung 4's shape put "anything else" on the harmless arm; rung 5a has no harmless arm of its own, so "anything else" arms nothing at all |
| unset | none | unchanged boot |

- Cargo feature `ga10bprobe5a`, **DEFAULT OFF**, implying `tegra` and reusing rung 4's helpers
  (`pg_state` / `clk` / `settle_ms` / `r32` / `w32` / the 4b write helpers) so the two ignitions
  cannot drift apart. No other configuration compiles a BCR write path — the invariant
  `ga10bprobe3b` established and rung 4 preserved.
- Witness family `[ga10bprobe5a]` — 15 bytes bracketed, over the 8-byte LLVM immediate-encode floor,
  and `LC_ALL=C grep -a -o -F` must find it in the flight artifact (LAWS §5, full-knob gate).
- **The three paths are named in the knob's own vocabulary line**, printed before the first read:
  `/GA10B/acr-gsp.text.encrypt.bin.prod`, `/GA10B/acr-gsp.data.encrypt.bin.prod` and
  `/GA10B/acr-gsp.manifest.encrypt.bin.out.bin.prod`, each with the sha256 the ledger row expects.
  A path the rung does not print is a path it does not read.
- **REFUSE with zero MMIO** if any of the three files is absent, unreadable, or digests differently
  from the ledger row (P6), if a placement is unaligned (P7), or if the window is too small (P8).
  Zero MMIO is literal: the refusal paths run before the BPMP bracket, so a REFUSED boot has not
  touched BAR0 and has not spent the power cycle.
- Announce discipline inherited verbatim from rungs 3b and 4: `about-to-read` lower-case,
  `about-to-WRITE` upper-case, every access announced before it is issued, so a fatal access names
  itself as the capture's last line.

### 5.7 The staging shape on the bench

The files travel as **ordinary files on the boot volume**, which is what makes the rest of the
tooling need no special case.

```
~/unaos-bench/flash/orin/<round>/GA10B/acr-gsp.text.encrypt.bin.prod
~/unaos-bench/flash/orin/<round>/GA10B/acr-gsp.data.encrypt.bin.prod
~/unaos-bench/flash/orin/<round>/GA10B/acr-gsp.manifest.encrypt.bin.out.bin.prod
~/unaos-bench/flash/orin/<round>/GA10B/LICENCE.txt
```

- **`GA10B/` is a directory at the FAT root**, beside `EFI/`, `kernel.elf`, `SRC.TGZ` and `SRC.SHA`.
  The kernel reads `GA10B/<name>` from the volume it bound; nothing else in the boot knows the
  directory exists.
- **`LICENCE.txt` is the agreement, copied unmodified** from the package's own
  `usr/share/doc/nvidia-l4t-firmware/copyright` (§4.2). It is staged because R52 requires the
  firmware to sit *beside the vendor's licence agreement*, and because clause 1.1(d) requires the
  agreement to reach each recipient of the software — a card is a recipient.
- **Every file is a MANIFEST row**, in the staged MANIFEST's existing shape (`<sha256>  <path>`, two
  spaces, one row per file, comment lines above):

  ```
  # GA10B firmware staged under R52: NVIDIA nvidia-l4t-firmware 36.4.3-20250107174145, unmodified.
  # Licence: NVIDIA Driver License Agreement, staged verbatim as GA10B/LICENCE.txt.
  <sha256>  GA10B/acr-gsp.text.encrypt.bin.prod
  <sha256>  GA10B/acr-gsp.data.encrypt.bin.prod
  <sha256>  GA10B/acr-gsp.manifest.encrypt.bin.out.bin.prod
  <sha256>  GA10B/LICENCE.txt
  ```

- **`make-pi-img.sh` and `media-writer.sh` carry them as ordinary files.** Neither needs a change:
  the image builder copies the staged directory, and the writer's sha-verify already reads the
  MANIFEST and re-hashes what it wrote. `tools/validate-manifest.py` likewise — it walks the staged
  directory and reds an orphan, which is exactly the check that catches a firmware file copied in
  and never listed.
- **The ledger row for the flight names each file, its size, its sha256 and the licence**, filled
  from the BOOT's printed digests, not from the staging host's — that is the difference between
  "what we put on the card" and "what the kernel read back", and only the second one is evidence.
- **Nothing here enters git.** The staged directory is outside the repository; the repository gains
  a ledger row of names and digests, which is text.

---

## 6. Questions only Peter can answer

These are asked, not guessed. Nothing in §5 is implemented before the ones marked LIVE are answered.
The numbering is kept from the first edition so that ledger A61 and the reports that cite Q1–Q5 still
point at the same questions; two are now closed and say so, and three new ones follow them.

1. **ANSWERED — the blob rule itself.** Ruled in by **R52** (2026-09-12): LAWS §3 gains one
   carve-out for a vendor firmware image staged unmodified on the board's media, loaded as data,
   never linked and never committed. §4.3 records the ruling and the obligations it carries. Nothing
   further is asked here.

2. **LIVE — is rung 5a flown, and attended?** It spends a power cycle by design, and fail shape
   **F21** — a fabric RAS from a real image whose DMA reach we do not bound — is a harder version of
   the risk that has taken this box down most often. Attended at the bench, with a manual power cut
   acceptable?

3. **ANSWERED IN PART — what is enumerated, and by whom?** The **names** are now enumerated without
   touching a rootfs or a blob: §4.1.1, from the public file list of a package whose upstream version
   string is NVIDIA's own, with the three the boot ROM wants identified in §4.1.2. What is still
   open is the part that needs bytes: the **sizes** and the per-file digests, which arrive only with
   the download — and that is Q6, not this question.

4. **LIVE, and now a precondition rather than an option — the cheap experiment.** §2.6: re-fly rung 4
   unchanged except that the six BCR DMA addresses are written `pa >> 8`. One boot, one power cycle,
   no new fact, no new address class, no blob. It was the only rung on this page free of Q1; it is
   now also the rung **rung 5a is not built before** (§5.1 P5x), because a 5a that flies first cannot
   attribute its own result. **Fly it next?**

5. **LIVE — the second door.** `bcr_ctrl = 0x011` — `BRFETCH = FALSE` — is in the ACKED SEQ and has
   never been tried (§2.2). It is a datum nobody has spent a boot on, and this seat has no theory
   about what it does. Worth a boot, or noise?

6. **NEW, LIVE — the download, which is yours to run.** §4.1.3 names one line: NVIDIA's own
   `nvidia-l4t-firmware_36.4.3-20250107174145_arm64.deb`, 2,295,852 bytes, SHA256 published by NVIDIA
   in a plain-text index, carrying both the `ga10b` directory and the agreement. No seat fetches it:
   this executor read manifests only. Three things to rule on, and they are one decision: **(a)** do
   you run that line, **(b)** at **36.4.3** — the version this tree's own ACKED facts were extracted
   from — or at 36.4.4 / 36.4.7, which are equally published and whose relevance to a signature is
   UNKNOWN, and **(c)** does a seat then get the `ls -l` and `sha256sum` output of the three files to
   put in the ledger row, or do you place them on the card yourself and hand the row over?

7. **NEW, LIVE — the second triple.** `safety-scheduler.{text,data,manifest}` (§4.1.2) has the same
   three-part shape as `acr-gsp` and is a second candidate payload for the very same registers. If
   `acr-gsp` returns F19 — a real image the ROM still refuses — is the second triple worth the next
   power cycle, or does the ladder stop and report? Asked now, before a failure makes it tempting to
   decide in the moment.

8. **NEW, LIVE — what travels with a card.** Clause 1.1(d) conditions redistribution on the
   agreement reaching each recipient (§4.2). §5.7 stages `GA10B/LICENCE.txt` on the card, which
   covers a card handed to a person. A card is not the only way this leaves the bench: is a flight
   image with vendor firmware on it ever to be published, mirrored or handed on at all — or does the
   staged firmware stay on bench media only, with the repository carrying nothing but names and
   digests? The strict reading costs nothing today and is the one this design assumes.

---

## 7. Verification posture of this document

Per rung 4 §9 and orin 22 BULLETIN §14 as sharpened by pi 9: a gate is skipped only when the change
cannot affect **what the gate asserts**, and the assertion must be **named**.

**What this arc changes.** This document and one ledger row (A61). **No `.rs` file is touched.** The
first edition of this brief added one new Markdown file and a one-line pointer to another; this
edition adds §4.1.1–§4.1.3, §4.3's ruling table, §5.1's P5x–P8, §5.2.1, §5.6, §5.7, bounding rows M
and N, fail shapes F27–F30 and §6's Q6–Q8. The kernel batteries assert properties of
compiled kernel images — `arroyo check`'s per-leg type-check, the QEMU specs' runtime witnesses,
`kernel8.img` byte identity. **No assertion any of them makes can be affected by a file the build
does not read.** This is the "no `.rs` file touched" safe form, not a bare "n/a", and specifically
not the rmbp B94 case (a comment inside a `.rs` file moves `panic::Location` line numbers and
therefore image bytes; neither file this arc touches is a `.rs`).

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
- §4.1.1's enumeration names its population too: `the arm64 file list of Ubuntu package
  linux-firmware-nvidia-tegra 36.4.3-20250107174145-0ubuntu1, read 2026-09-12; entries under
  nvidia/ga10b/ = 17`. The version string is NVIDIA's own, which is what makes a distributor's
  listing usable as an enumeration of NVIDIA's set rather than of the distributor's taste.
- §4.1.3's sizes were read from an NVIDIA-published plain-text package index and confirmed by HTTP
  HEAD requests, which return a `Content-Length` and no body. **No firmware, tarball, deb or other
  binary was downloaded by this seat** — the deliberate boundary of this task.
- §4.2's licence clauses were read in the agreement's own full text, not from a summary page. That
  reading **corrected** the first edition's "1.1(4)" to **1.1(d)** and added clauses 2.6 and 2.9,
  both of which bear on this ladder.

**What was NOT measured, stated so no reader can lose it.**

- **Nothing was run on hardware.** No boot, no flight, no register was touched by this arc.
- `NV_PRISCV_RISCV_BR_RETCODE` was **not found** in the three published MIT headers searched for it
  by name (GA102, GH100, GB100). That is "not found in that scope", not "does not exist".
- **The GA10B firmware file SIZES are still not known** (§4.1), and neither is any file's sha256. No
  JetPack rootfs was read and no package was opened; the names come from a package listing.
- **The mapping of the three `acr-gsp` files onto `fmccode`/`fmcdata`/`pkcparam` is an INFERENCE**
  (§4.1.2), from file names plus §1.3's roles. It is the load-bearing inference of §5 and it is not
  measured on this die or in any source read here.
- **Whether 2 MiB is enough for the three sections is UNKNOWN** and is why §5.1 P8 exists.
- Whether GA10B's boot ROM shares Hopper's address encoding was **not measured** and is §6 Q4.
- No source in this document has been pointer-verified by a Group A pass, and none of it may gate
  code until one does. **This brief gates nothing: it proposes no code.**
