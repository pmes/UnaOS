# WIFI-1 — the firmware-load arc of the BCM4331 ladder

Status: **arc 1 landed (firmware load). Compile-verified only — no metal boot yet.**

This document covers one arc. The subsystem it belongs to — the BCM4331 native-driver ladder S0..S8,
its metal captures, and the §S4 licensing decision box — is
[`bcm4331.md`](bcm4331.md), and that document is authoritative wherever the two touch.

The radio is a Broadcom BCM4331 (`14e4:4331`), the AirPort Extreme part in the 2012 15" retina
MacBook Pro. It is a SoftMAC device: the on-chip d11 core executes microcode the host must upload
before the radio can do anything at all. Getting that microcode off the user's media and into kernel
memory is what arc 1 builds.

## Legal posture

`docs/MANIFESTO/CLEAN_ROOM_POLICY.md` governs this work.

* **UnaOS ships no firmware.** §4: proprietary assets are supplied by the user at runtime. The
  loader reads files the user placed on the media. Nothing is bundled, nothing is fetched, and per
  `bcm4331.md` §S4 the blob "must never be committed to this repository and must never be baked into
  a media image we distribute". That is §S4's option 1, and this arc implements exactly it.
* **Firmware is loaded, never authored.** GR22 and `bcm4331.md` §S4 both settle that. §S4 is careful
  about *why*: the d11 PSM is unsigned, so authoring is not blocked by cryptography — it is blocked
  by the absent rev-29 ISA document, the size of a real-time MAC, and the HT-PHY wall standing behind
  it. Not our route either way.

### The clean-room claim, scoped

**The claim is about `src/wifi/` and its implementer, not about the WiFi subsystem.** These three
files were written without reading any GPL Linux WiFi driver source. Their factual inputs are the
public PCI base specification, the public PCI-SIG vendor registry, and this tree's own metal captures
and prose in `bcm4331.md`.

That is **not** the sourcing of the sibling module, and this document does not launder it:

* `drivers/bcma.rs` states in its own header that its register offsets and EROM encodings "follow
  Linux `drivers/bcma` (`bcma_regs.h`, `bcma_driver_chipcommon.h`, `scan.c`) and `b43`'s
  `B43_MMIO_PHY_VER`".
* `bcm4331.md` §S4 states that its upload sequence is "transcribed from the b43 reference
  implementation's *interface*".

Anyone extending `src/wifi/` across that boundary — which arc 2 does the moment it walks the core
table — inherits `CLEAN_ROOM_POLICY.md` §2's two-team rule and should record which side they are on.

## The knob

`UNAOS_WIFI=1` arms the `wifi` Cargo feature. Default **OFF** — the module and its three call sites
vanish and every image is byte-identical to baseline. When on, `wifi` appears in the
`⚡ kernel features:` banner.

| Place | Entry |
| --- | --- |
| `unaos/crates/kernel/Cargo.toml` | `wifi = []` |
| `unaos/crates/kernel/src/lib.rs` | `#[cfg(all(feature = "wifi", target_arch = "x86_64"))] pub mod wifi;` |
| `unaos/crates/kernel/src/main.rs` | `wifi::service()` at **all three** storage-ready loop passes |
| `unaos/arroyo` (feature mapping) | `UNAOS_WIFI=1` → `wifi` |
| `unaos/arroyo` (`arm_features`) | stripped for aarch64 — x86-only code, so leaving it enabled would shift Pi/Jetson media hashes for zero observable change (the `sdw`/`kbdwit`/`pcicensus`/`bcmarecon` argument) |
| `unaos/arroyo` (`KERNEL_CFG_MATRIX`) | appended to the `x86-all` leg; the `x86-mix-*` legs derive from that union |
| `unaos/builder/src/main.rs` | `UNAOS_WIFI` → `wifi` |

The builder wiring is not optional, for the reason `bcm4331.md` §4 gives: the builder re-derives the
x86 feature set from env, so a knob wired only in `arroyo` ships the loader disabled while the banner
claims it is on. A loader that silently did not run is indistinguishable on the wire from media with
no firmware on it.

**Three call sites, not one.** `main.rs` carries three storage-ready loop passes and which one a given
x86 build reaches depends on its knobs — the same reason `shell::fatverb_storage_witness` sits at all
three. The forward-only state machine makes the path speak exactly once whichever one runs.

QEMU models no BCM4331, so a QEMU run can only ever witness the honest no-radio refusal. This knob is
metal-first by construction; a green `./arroyo check` proves it compiles, not that it works.

## The ladder

| Arc | Scope | Writes the device? |
| --- | --- | --- |
| **1 (landed)** | Config-space identification of the radio, cross-checked against pinned metal facts + locate/validate/stage the firmware set from the program-source volume | **No.** Config-space reads and FAT reads only. |
| 2 | Map BAR0, walk the bcma core table, reach the d11 core and its wrapper, run §S4's reset prologue, stream the staged microcode through the SHM indirect window | Yes — the first device write in this module |
| 3 | PHY/radio init from the staged initvals, a receive path, a scan, one authenticate/associate exchange, bound to `smolnet` through the existing `net_phy` seam | Yes |

Arc 2's mechanics are `bcm4331.md` §S1c/§S3/§S4, not this document.

## Arc 1 — what the code does

`unaos/crates/kernel/src/wifi/`

* `mod.rs` — a forward-only state machine driven from the x86 main loop. `S_START` runs the census
  once; a refusal parks permanently. The radio found moves to `S_WAIT_STORAGE`, which announces the
  deferral once and polls for a program-source block device. One staging pass, then `S_PARKED`
  forever. **There is no path back to an earlier state and no path out of `S_PARKED`**, so a failure
  cannot become a retry storm.
* `bus.rs` — PCI **config-space** sweep for the radio. Strictly read-only: no BAR is mapped, no MMIO
  is touched, no register is written, bus-master is not enabled, `cfg:0x80` is not touched.
* `firmware.rs` — locates the firmware set on the program-source volume, bounds-validates each file,
  classifies its container header, and stages the bytes in kernel-owned buffers (`staged_count()` /
  `with_staged(role, f)`). FAT reads only — never a sector write, never a directory entry, never a
  FAT mutation.

### The subclass is the whole point

The BCM4331 reports PCI class 0x02 **subclass 0x80** ("other network controller"). The bench rMBP
also carries the BCM57765 combo chip at `3:0.x`, whose Gigabit Ethernet MAC is class 0x02 **subclass
0x00** and sits at a *lower* bdf than the radio at `4:0.0`. Matching on class alone and taking the
first hit therefore selects the Ethernet MAC of a laptop with no Ethernet jack.

That is not hypothetical. It is the defect `bcm4331.md` §0 records: `PciScanner::find_device(0x02,
0x00)` "could never have matched" the radio, and the part "has never been examined by this OS"
because of it. `bus.rs` matches `class == 0x02 && subclass == 0x80`, counts subclass-0x00 Broadcom
functions separately, and when those are the only ones present it refuses **by name** rather than
silently selecting one.

**Duplication with `drivers/bcma.rs`.** `bcma::find_wifi()` performs the same sweep with the same
discriminator. It is not reused because it is private to a module gated on `bcmarecon`, and that
feature also arms a BAR0 mapping and MMIO recon — arming it from `UNAOS_WIFI=1` would make this arc's
"no MMIO, no device write" claim false by construction. The two sweeps are deliberately independent
and deliberately agree on the discriminator; **if one is changed the other must be.**

### The census is a cross-check, not a first look

The radio's identity is already pinned on metal. `bcm4331.md` §0 records it from Boots AF and AJ,
three boots: `4:0.0`, `14e4:4331`, `rev=02`, `ssid=106b:00ef`, `bar0=0xc1900000` type `mem64`
non-prefetchable `maplen=0x2000`, `cmd=0x0006` (memory decode + bus master on), power state D0,
behind bridge `0:28.1`, `matches=1` on the whole machine.

An instrument that merely printed what it saw could not fail. So `bus.rs` compares each field against
those recorded values and prints `MATCH` or `MISMATCH` per field, plus a `cross-check=PASS|FAIL`
verdict on the selection line. A MISMATCH is a real finding — a different machine, a different card,
or firmware that no longer parks the part as it did — and **arc 2 is gated on the cross-check
passing.**

### Staging the card — the set is three files

`bcm4331.md` §S4 pins what the d11 needs, from a core revision and PHY type measured over three
boots (`rev=29`, `phy type=7 (HT)`):

| role | canonical name | 8.3 alias | why |
| --- | --- | --- | --- |
| `ucode` | `ucode29_mimo.fw` | `UCODE29.FW` | microcode; b43 selects on d11 core revision, and rev 29 + HT-PHY maps to `ucode29_mimo`. A rev-26 or rev-30 image is the wrong blob. |
| `initvals` | `ht0initvals29.fw` | `HT0IV29.FW` | register init table, selected by PHY type **and** core rev |
| `bsinitvals` | `ht0bsinitvals29.fw` | `HT0BSI29.FW` | per-band retune table, same selection |

No PCM image — rev 29 does not use one. `B43.FW` is retained as a **legacy alias for the microcode
only** (WIFI-1's original single-blob name); it is not the documented staging name.

The loader reads through **`fat::mount_program_source()`**, not `fat::mount()` — the FAT-verb law is
that a "wherever I can find it" read follows the PROGRAM SOURCE, and firmware is exactly that class of
read. In practice that is the global block device when one is registered, else (x86 + `sdhcblk`) the
card in the internal SD reader; on a machine booted from that reader the global handle is empty while
a program-bearing volume is mounted, and `mount()` would find nothing. The block-device presence gate
in `mod.rs` asks the same question (`block::program_source()`), so gate and mount cannot disagree.

Directories searched, in order: **`/`, `/B43/`, `/FIRMWARE/`**. Within each, each role is tried by its
canonical name first, then its 8.3 alias — the canonical names are 14–19 characters and need VFAT
long-filename entries, which the reader supports (PI-FS-3) but a plainly-formatted volume may not
carry. The match is case-insensitive.

**First existing name wins, per role.** A file that exists but fails validation is REJECTED and the
loader does **not** fall through to the next alias or directory for that role: a present-but-wrong
`ucode29_mimo.fw` is a fact about the user's media that must be reported, not routed around. So a
128-byte `/B43.FW` rejects the `ucode` role outright and `/B43/ucode29_mimo.fw` is never reached.

Accepted size band: **256 B ≤ size ≤ 4 MiB** — §S4 gives "tens of KB for the ucode, a few KB each for
the initvals", so the floor clears a plausible small table and the ceiling leaves ~two orders of
magnitude of headroom while staying bounded.

Provenance is the user's: §S4 records the set as extracted from `broadcom-wl-5.100.138` with
`b43-fwcutter`, the same extraction every Linux distribution asks its users to perform for this card.

### What is validated, and how honestly

* **Presence** — pinned, reported per file.
* **Bounds** — pinned by us, not by the vendor. Outside the band is a hard REJECT, as is a short read
  (a cluster chain ending before the directory-recorded size). Half a microcode image pushed into the
  core is worse than no radio.
* **Word alignment** — pinned by §S4: "the payload is a stream of big-endian 32-bit words". Reported.
* **Container header — a documented disagreement, reported rather than resolved by fiat.** §S4
  records "a 4-byte header (`type`, `ver`, reserved, big-endian `size`)". Those four fields cannot fit
  in four bytes — a `be32` size alone is four — so the record is internally inconsistent and cannot be
  implemented as written. Rather than pick a reading and call it pinned, `classify_header` tries both
  self-consistent candidates and reports which one the file satisfies:

  * **Layout A (8-byte):** `type` u8, `ver` u8, 2 reserved, `be32 size`; holds iff `size == len - 8`.
  * **Layout B (4-byte):** `type` u8, `ver` u8, 2 reserved, no declared size; holds iff
    `(len - 4) % 4 == 0` and A does not hold.

  The accept decision rests only on presence and bounds, so a file in either container — or neither —
  still stages, and says which.

## Open UNKNOWN and its metal probe

Exactly one survives arc 1. W1/W2/W4 of the original draft (device id, BAR0 assignment, reachability
behind the bridge) were **not** unknowns: `bcm4331.md` §0 pins all three over three boots, and they
are now cross-checked rather than discovered.

| # | Unknown | Probe |
| --- | --- | --- |
| W3 | The firmware container header layout — §S4's own record is internally inconsistent (see above) | The `hdr=A\|B\|unrecognized`, `type=`, `ver=`, `declared=`, `words=` fields of each `STAGED` line on the first boot with the real set. That capture corrects §S4 and gates arc 2. |

## Witness lines

Every file gets one line, and the pass ends in **exactly one terminal verdict**. Some lines are
explicitly non-terminal (an unreadable or chainless search directory) and say so.

**Census** — clean:

```
:: wifi: brcm net function 03:00.0 device=0x1686 subclass=0x00 (Ethernet) — NOT the radio, skipped ::
:: wifi: radio 04:00.0 device=0x4331 expected=0x4331 MATCH rev=0x02 expected=0x02 MATCH subsys=0x106b:0x00ef expected=0x106b:0x00ef MATCH ::
:: wifi: radio 04:00.0 bar0=0x00000000c1900000 expected=0x00000000c1900000 kind=mem64 expected=mem64 MATCH ::
:: wifi: radio SELECTED 04:00.0 matches=1 expected=1 MATCH cross-check=PASS (vs bcm4331.md §0, Boots AF/AJ) — config-space read-only, no MMIO ::
```

Census — refusal / divergence forms:

```
:: wifi: REFUSED — 1 Broadcom class-0x02 function(s) found but ALL subclass 0x00 (Ethernet, the BCM57765 MAC); the AirPort radio is subclass 0x80 and is absent from this machine's config space — parked ::
:: wifi: no Broadcom (0x14e4) class-0x02/subclass-0x80 function in PCI config space (0 Ethernet-subclass Broadcom functions either) — parked ::
:: wifi: WARNING BAR0 unassigned on 04:00.0 — firmware left the window unallocated; arc 2 will need to size and assign it ::
```

A `MISMATCH` on any field, or `cross-check=FAIL`, uses the same line shapes with the other word.

**Firmware staging** — clean:

```
:: wifi: firmware staging deferred — no program-source block device yet (the set lives on that FAT volume) ::
:: wifi: ucode STAGED /B43/ucode29_mimo.fw bytes=94800 on source=global label='UNAOS' fp=0x1234abcd:0x0000f000 fnv1a=0xdeadbeef hdr=A type=0x75 ver=0x01 declared=94792 words=ok ::
:: wifi: initvals STAGED /B43/ht0initvals29.fw bytes=3096 on source=global … ::
:: wifi: bsinitvals STAGED /B43/ht0bsinitvals29.fw bytes=1224 on source=global … ::
:: wifi: firmware set COMPLETE 3/3 staged on source=global label='UNAOS' fp=… — held in kernel memory, NOT pushed to the core (no MMIO, no device write); arc 2 owns bcma core bring-up ::
```

Firmware staging — absent / failure forms:

```
:: wifi: ucode ABSENT — none of ucode29_mimo.fw|UCODE29.FW|B43.FW in /, /B43/, /FIRMWARE/ on source=global label='…' fp=… ::
:: wifi: firmware set INCOMPLETE 0/3 staged (0 rejected) on source=global … — missing: ucode(ucode29_mimo.fw|UCODE29.FW|B43.FW), initvals(…), bsinitvals(…) — parked, radio stays down ::
:: wifi: firmware NOT staged — program-source FAT volume would not mount (no FAT partition/BPB found); searched /, /B43/, /FIRMWARE/ ::
:: wifi: firmware NOT staged — root directory unreadable (block I/O error) on source=… ; searched /, /B43/, /FIRMWARE/ ::
:: wifi: ucode REJECTED /B43.FW size=12 on source=… — reason=too-small (min 256) ::
:: wifi: ucode REJECTED /B43/ucode29_mimo.fw size=8388608 on source=… — reason=too-large (max 4194304) ::
:: wifi: ucode REJECTED /B43/ucode29_mimo.fw size=94800 on source=… — reason=short-read (got 40960 bytes) ::
:: wifi: ucode REJECTED /B43/ucode29_mimo.fw size=94800 on source=… — reason=read-failed (corrupt FAT chain) ::
```

Non-terminal notes (the pass continues and still ends in one verdict):

```
:: wifi: /B43/ unreadable (corrupt FAT chain) — skipping that directory (non-terminal) ::
:: wifi: /B43/ has no cluster chain (first_cluster=0) — skipping that directory ::
```

## Clean-vs-failure summary

* **Clean, set present:** the Ethernet-skip line(s), two `radio` cross-check lines, one
  `radio SELECTED … cross-check=PASS`, three `STAGED` lines, one `firmware set COMPLETE 3/3`. No
  `MISMATCH`, no `REJECTED`, no `ABSENT`, no `REFUSED`.
* **Clean, set absent (the expected first metal boot, since blob-on-media is unconfirmed):** the
  census lines through `cross-check=PASS`, three `ABSENT` lines naming every accepted name and every
  searched directory, then one `firmware set INCOMPLETE 0/3 … parked`. Boot proceeds normally; the
  radio stays down; nothing repeats.
* **Failure:** any `MISMATCH`, `cross-check=FAIL`, `REFUSED`, `REJECTED`, or `NOT staged` line, each
  carrying its own reason. A boot that prints no `wifi:` lines at all means the knob was not armed —
  check the `⚡ kernel features:` banner for `wifi`, and check `builder/src/main.rs` if the media was
  produced by the builder rather than by `arroyo`.
