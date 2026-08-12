# WIFI-1/WIFI-2 — the firmware-load and core-bring-up arcs of the BCM4331 ladder

Status: **arc 1 landed and FLOWN (Boot A — the identity cross-check passed on metal). Arc 2 landed,
compile-verified only, awaiting its first metal boot. WIFI-REARM landed on top of both
(compile-verified; its metal proof is Boot B).**

This document covers two arcs. The subsystem they belong to — the BCM4331 native-driver ladder
S0..S8, its metal captures, and the §S4 licensing decision box — is
[`bcm4331.md`](bcm4331.md), and that document is authoritative wherever they touch.

The radio is a Broadcom BCM4331 (`14e4:4331`), the AirPort Extreme part in the 2012 15" retina
MacBook Pro. It is a SoftMAC device: the on-chip d11 core executes microcode the host must upload
before the radio can do anything at all. Getting that microcode off the user's media and into kernel
memory is what arc 1 builds; reaching the core that would execute it — and proving we have reached
the right one — is what arc 2 builds.

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

**The claim covers arc 1's three files and NOT arc 2's.** `mod.rs`, `bus.rs` and `firmware.rs` were
written without reading any GPL Linux WiFi driver source and without adopting code or constants from
`drivers/bcma.rs`; their factual inputs are the public PCI base specification, the public PCI-SIG
vendor registry, and this tree's own metal captures and prose in `bcm4331.md`.

**`bringup.rs` is on the Group-B side, by adoption, and this document previously claimed otherwise.**
The claim "nothing was copied from `drivers/bcma.rs`" was textually false: ~59 constants are that
module's, name for name and value for value — including `EROM_MAX_CORES = 32`, a print budget rather
than a property of any silicon and so not something two implementers arrive at independently — and
the `Erom` struct with its `new`/`stopped`/`peek`/`take`/`ci`/`at_end`/`master_port` accessors is that
module's executable code. Only the parse strategy (tag-driven descriptor consumption in
`Erom::address` and `walk_erom`) is independent. The false sentence is withdrawn from the commit
message, from `mod.rs` and from `bringup.rs`'s header, and the inherited taint is recorded below
instead. A recorded taint costs nothing; a laundered one costs the credibility of every other claim
in the subsystem.

That is **not** the sourcing of the sibling module, and this document does not launder it:

* `drivers/bcma.rs` states in its own header that its register offsets and EROM encodings "follow
  Linux `drivers/bcma` (`bcma_regs.h`, `bcma_driver_chipcommon.h`, `scan.c`) and `b43`'s
  `B43_MMIO_PHY_VER`".
* `bcm4331.md` §S4 states that its upload sequence is "transcribed from the b43 reference
  implementation's *interface*".

Anyone extending `src/wifi/` across that boundary — which arc 2 does the moment it walks the core
table — inherits `CLEAN_ROOM_POLICY.md` §2's two-team rule and should record which side they are on.
**Arc 2 is on the Group-B side and records it**, per fact, in the provenance ledger at the end of this
document and at every constant in `bringup.rs`.

## Boot A — what arc 1 actually did on metal

Arc 1's first flight, `~/unaos-bench/capture/gr25-bootA/ttyUSB0.log` (read with `awk`, never `grep`):

```text
:: wifi: brcm net function 03:00.0 device=0x16a3 subclass=0x00 (Ethernet) — NOT the radio, skipped ::
:: wifi: radio 04:00.0 device=0x4331 expected=0x4331 MATCH rev=0x02 expected=0x02 MATCH subsys=0x106b:0x00ef expected=0x106b:0x00ef MATCH ::
:: wifi: radio 04:00.0 bar0=0x00000000c1900000 expected=0x00000000c1900000 kind=mem64 expected=mem64 MATCH ::
:: wifi: radio SELECTED 04:00.0 matches=1 expected=1 MATCH cross-check=PASS (vs bcm4331.md §0, Boots AF/AJ) — config-space read-only, no MMIO ::
:: wifi: ucode ABSENT — none of ucode29_mimo.fw|UCODE29.FW|B43.FW in /, /B43/, /FIRMWARE/ on source=sdhc label='UNAOS-X86' fp=0xc27415fd:0x001dbb43 ::
:: wifi: firmware set INCOMPLETE 0/3 staged (0 rejected) on source=sdhc … — parked, radio stays down ::
```

Two readings, and they set arc 2's shape:

* **The identity gate is PASS.** Every cross-checked field matched, `matches=1`, and the Ethernet
  skip line fired — so the subclass discriminator did the job it exists for on a machine that really
  does carry both parts. Arc 2 is gated on exactly this line and is therefore unblocked.
* **The firmware gate is 0/3.** The set was not on the card. W3 (the container-header disagreement)
  is therefore still open: no `STAGED` line has ever printed an `hdr=` verdict.

One line was NOT predicted by the arc-1 notes and is worth recording: the Ethernet function reported
`device=0x16a3`, where `bcm4331.md` §0 names `14e4:16bc` (the SDXC reader at `3:0.1`) and the
Gigabit MAC at `3:0.0`. `0x16a3` is that MAC's own device id, read for the first time. It changes
nothing — the discriminator is the subclass, not the device id — but the census is the first
instrument in this tree to have printed it.

## WIFI-REARM — Boot D, and the silence that could not be falsified

Boot D (`gr26-bootD`) flew arc 1 again and the census was clean: `radio SELECTED 04:00.0 matches=1
cross-check=PASS` at 14.774 s. One millisecond later:

```
[  14775ms] :: wifi: firmware staging deferred — no program-source block device yet (the set lives on that FAT volume) ::
```

and then **not one further `wifi:` line for the remaining nine minutes of uptime**. Nothing ever
enumerated on that boot — the same capture carries
`xHCI: … note='no mass-storage device enumerated'`, a `U9x` storage bound expiring at 29.8 s, and
`desktop-app DECLINE reason=no-storage waited=30000ms` — so there was genuinely nothing to stage, and
the storage-lane defect is a separate arc.

The WiFi-side defect is a different one and it is about EVIDENCE. Arc 1's `S_WAIT_STORAGE` arm did
re-poll `block::program_source()` on every service pass; it simply never said so. A service still
polling and a service that has given up produce byte-identical serial — silence — so the capture
cannot distinguish them, and the arc-2 walk sitting behind that gate had no witness saying why it had
not run. An instrument that reads the same whether its mechanism is alive or dead is not evidence of
either.

### What changed

**1. The wait speaks, on a bounded schedule.** The deferral line still fires once and still carries
its original text (Boot D's line, extended with the promise the heartbeats keep). While the volume is
absent, a heartbeat prints every `WAIT_SPEAK_MS` = 10 s, up to `MAX_SPOKEN_WAITS` = 6 — the first
minute, well past the ~34 s at which Boot D's xHCI reported it had enumerated nothing. The sixth
heartbeat says the poll continues silently, and the poll DOES continue: nothing is disarmed, no state
is stored on that branch.

**2. The wait stays unbounded.** The heartbeat COUNT is capped; the wait is not. A program-source
volume can appear at any time — a stick plugged in after boot, a storage lane that comes up late —
and there is no instant at which "it will never arrive" becomes true. A volume arriving at minute
nine is still announced and still staged.

**3. A staging attempt that hits an unsettled transport is re-armed, not spent.** A registered handle
is not a settled transport: the block device is published by the xHCI storage bring-up, and the first
mount through it can return `NoDisk`/`Io`/`Busy` while that bring-up finishes. `stage_attempt` now
classifies its own failure:

| outcome | when | what the caller does |
| --- | --- | --- |
| `Settled` | the volume mounted and every role got its verdict — **or** the mount failed for a reason a later pass cannot change (`NotFat`, `Unsupported`, a corrupt chain) | prints nothing more; runs arc 2; parks |
| `Retry(stage)`, budget left | mount or root-directory read failed with `NoDisk`/`Io`/`Busy` | backs off `STAGE_BACKOFF_MS` = 1 s, re-checks the handle from the top, re-attempts; up to `MAX_STAGE_ATTEMPTS` = 8. Does **not** run arc 2 — the count is still moving |
| `Retry(stage)`, budget spent | the 8th attempt also deferred | prints the give-up line; the deferral is now terminal, so `staged_count()` (0) is final; runs arc 2; parks |
| `Pending` (WIFI-REACH) | every volume present was searched, the set is incomplete, and **no second handle** existed to search | prints nothing terminal; moves to `S_WAIT_ALT` and holds arc 2 for a late second volume — see below |

A volume that is not FAT now will not be FAT in two seconds, so those variants stay terminal;
retrying them would be a spin dressed up as diligence. Both `Retry` arms sit **above** the
`for spec in FW_SET` loop, so a retry can never double-stage a role or resume a half-built set —
that is a property of where the arms are, and it is why `STAGED` needs no reset between attempts.

**4. The bound fails out loud.** Eight deferred attempts against a present handle print a named
give-up line, run arc 2 on the settled `staged_count()==0`, and park. The failure mode of a bounded
retry must be a printed line, never a quiet downgrade back to the silence this change exists to
remove.

### What deliberately did NOT change

Arc 2 still runs only after the staging pass has reached its **terminal** answer for the boot — a
`Settled` outcome, or the retry budget exhausted. Both leave `staged_count()` final before the
completeness gate reads it (the exhaustion path stages nothing, by where the `Retry` arms sit), and
the *non-terminal* `Retry` is the one case held back. Pre-WIFI-REARM there was no non-terminal
outcome and arc 2 ran after every staging pass, so the exhaustion path preserves exactly the old
behaviour for what used to be a single failed mount; nothing about arc 2's own gating was loosened.
A boot whose volume never appears still never reaches arc 2 — it waits, now visibly. Releasing arc 2
on a timer was considered and rejected
for the two reasons the original call site records: it would put a backplane window write in a race
with the pass that decides whether an upload may follow, and it would have arc 2's completeness gate
read `staged_count()==0` as final on a boot where the volume then arrives. WIFI-REARM makes that wait
honest, not shorter.

The upload rung is untouched. `B43_SHM_UCODE` remains an open UNKNOWN and R7 still refuses.

### Expected Boot B witness chain

Storage never appears (Boot D's shape, now legible):

```
:: wifi: radio SELECTED 04:00.0 … cross-check=PASS … ::
:: wifi: firmware staging deferred — no program-source block device yet (the set lives on that FAT volume); re-checking every pass, staging re-arms whenever it appears ::
:: wifi: still deferred n=1/6 waited=10000ms — no program-source volume yet (handles=global=absent sdhc=unbuilt); the poll is LIVE ::
…
:: wifi: still deferred n=6/6 waited=60000ms — … the poll is LIVE — last heartbeat: the poll continues SILENTLY from here and an arrival is still announced and staged ::
```

Storage appears late (the outcome the storage-lane arc is chasing):

```
:: wifi: firmware staging deferred — … re-checking every pass, staging re-arms whenever it appears ::
:: wifi: still deferred n=1/6 waited=10000ms — … the poll is LIVE ::
:: wifi: program-source volume APPEARED after waited=17431ms (handles=global=present sdhc=unbuilt) — resuming firmware staging at attempt n=1/8 ::
:: wifi: ucode STAGED … / firmware set COMPLETE 3/3 … (or the ABSENT/INCOMPLETE forms)
:: wifi2: begin — arc 2: map BAR0, walk the EROM … ::
… the normal arc-2 chain through its one `end` line …
```

Storage appears but its transport has not settled:

```
:: wifi: program-source volume APPEARED after waited=11002ms (handles=global=present sdhc=unbuilt) — resuming firmware staging at attempt n=1/8 ::
:: wifi: staging attempt DEFERRED at mount — program-source volume present but would not mount (block I/O error); nothing staged, re-attempting ::
:: wifi: staging re-armed — attempt n=1/8 deferred at stage=mount, next attempt in 1000ms ::
:: wifi: staging attempt DEFERRED at mount — … ::
:: wifi: staging re-armed — attempt n=2/8 deferred at stage=mount, next attempt in 1000ms ::
… (the arrival line does NOT repeat — it reports the arrival, not the attempt; `n=` on the re-arm
    lines is what counts the attempts) … and either a staging verdict, or, after eight:
:: wifi: firmware NOT staged — 8 attempts all deferred at stage=mount against a present handle (handles=global=present sdhc=unbuilt); giving up staging for this boot ::
```

A boot whose volume is present at the first check prints **no** arrival line and stages on the pass
that saw it — the pre-WIFI-REARM timing, unchanged, because `NEXT_ATTEMPT_MS` is 0 until an attempt
actually defers.

## WIFI-REACH — the SECOND-handle wait, so a late stick is actually searched

WIFI-REARM waits for the FIRST volume, the program source. PSRC then widened the firmware search to a
SECOND volume — the other populated handle — so that b43 blobs the user carries on a USB stick are
found even when the boot volume is the internal card. But that second pass only fires when BOTH
handles are present at attempt time, and on the bench they are not: the internal SD card is registered
synchronously inside `pci::init`, before the main loop runs, so `program_source()` is already
`Some(Sdhc)` on main-loop pass 1 and the staging entry fires there — while the USB stick enumerates
many passes later, from the deferred SCSI bring-up. At that first attempt `alternate_program_source()`
is `None`, pass 2 finds no alternate, and the pre-WIFI-REACH code printed a terminal INCOMPLETE and
ran arc 2 an epoch before the stick could be looked at. That is the OPEN DEFECT `sdhc.md` §13.7 named
(card-early / stick-late), and WIFI-REACH closes it.

**The mechanism.** `stage_attempt` takes a `commit` flag and gains a third outcome:

| outcome | when | what the caller does |
| --- | --- | --- |
| `Pending` | `commit == false`, the set is incomplete, and **no** alternate handle was present to search | prints nothing terminal; `service` moves `S_WAIT_STORAGE` → `S_WAIT_ALT` and holds arc 2 |

On the committing attempt (`commit == true`) the verdict is FORCED — COMPLETE or INCOMPLETE — so
`Pending` is never the last word. In `S_WAIT_ALT` the module holds until either:

* the **USB storage-ready edge** fires — `block::set_usb_ready()` is raised by
  `publish_usb_geometry` when the stick enumerates, and `block::take_usb_ready()` consumes it. That
  edge has NO other x86 consumer (its only consumer, `fat::piusb27_service`, is
  `#[cfg(target_arch = "aarch64")]`, and `wifi` is x86-only), so the two live on disjoint arches and
  cannot race — `wifi::service` consumes it freely. On the edge the two-volume search re-runs, now
  with the stick as the alternate, and settles; or
* a **bounded deadline** `ALT_WAIT_MS` = 30 s expires — past the ~34 s at which Boot D's xHCI had
  already reported it enumerated nothing — at which point the committing attempt forces the honest
  INCOMPLETE and arc 2 runs on the (still starved) count, refusing exactly as before.

The state machine stays strictly forward-only: `S_START` → `S_WAIT_STORAGE` → `S_WAIT_ALT` →
`S_PARKED`, no step back, no path out of `S_PARKED`. The two invariants both prior arcs rest on hold
by construction: **exactly one terminal verdict** (the committing attempt always prints it, `Pending`
prints nothing) and **arc 2 runs once on a final count** (only from `finish_and_park`, never on a
`Pending`/`Retry` that is still moving). The upload refusal at `B43_SHM_UCODE` in arc 2 (R7) is
untouched — WIFI-REACH changes only WHEN the terminal verdict is reached, never what arc 2 does.

### Expected witness chain — the bench, stick carrying the blobs

The whole point of the arc: dropping the three blobs on a USB stick and booting from the card now
finds and stages them, no re-flash of the boot card.

```
:: wifi: radio SELECTED 04:00.0 … cross-check=PASS … ::
:: wifi: ucode ABSENT — none of ucode29_mimo.fw|UCODE29.FW|B43.FW in /, /B43/, /FIRMWARE/ on source=sdhc … ::
:: wifi: initvals ABSENT — … on source=sdhc … ::
:: wifi: bsinitvals ABSENT — … on source=sdhc … ::
:: wifi: firmware set INCOMPLETE on the program source and NO second populated handle present yet — HELD up to 30000ms for a late-publishing volume (e.g. a USB stick carrying the b43 set); terminal verdict and arc 2 DEFERRED until its storage-ready edge fires or the deadline expires ::
:: wifi: second-handle wait n=1/6 — set still incomplete, no alternate handle yet (handles=global=absent sdhc=present); ~20000ms until the terminal verdict is forced ::
:: wifi: second-handle wait — USB storage-ready edge fired (handles=global=present sdhc=present); re-running the two-volume firmware search for the missing roles ::
:: wifi: firmware set incomplete on the program source (0/3) — searching the other populated handle (source=global) for the missing roles; READ-ONLY, and a role already staged is never replaced ::
:: wifi: ucode STAGED /B43/ucode29_mimo.fw bytes=… on source=global label='…' fp=… ::
:: wifi: initvals STAGED … on source=global … ::
:: wifi: bsinitvals STAGED … on source=global … ::
:: wifi: firmware set COMPLETE 3/3 staged on source=sdhc … + source=global … ::
:: wifi2: begin — arc 2: map BAR0, walk the EROM … ::
… the normal arc-2 chain through its one `end` line …
```

If the stick never appears, the hold heartbeats to the deadline and then commits the honest verdict:

```
:: wifi: firmware set INCOMPLETE on the program source and NO second populated handle present yet — HELD up to 30000ms … ::
:: wifi: second-handle wait n=1/6 — … ~20000ms until the terminal verdict is forced ::
… up to n=6/6 or the deadline …
:: wifi: firmware set INCOMPLETE 0/3 staged (0 rejected) on source=sdhc … — missing: ucode(…), initvals(…), bsinitvals(…) — parked, radio stays down ::
:: wifi2: begin … ::  (arc 2 refuses on the starved count, exactly as before)
```

A boot where BOTH handles are already present at the first attempt (e.g. booted from the stick, card
registered early) never enters `S_WAIT_ALT` — pass 2 fires on the first attempt and the verdict is
terminal there. `Pending` is specific to the incomplete / no-second-handle-yet case.

## The knobs

`UNAOS_WIFI=1` arms the `wifi` Cargo feature (arc 1). Default **OFF** — the module and its three call
sites vanish and every image is byte-identical to baseline. When on, `wifi` appears in the
`⚡ kernel features:` banner.

`UNAOS_WIFI2=1` arms `wifi2` (arc 2), which **implies `wifi`**. Default **OFF**. The split is by
WRITE, not by convenience: everything arc 1 does is a PCI-config read or a FAT read, while every rung
in arc 2 either writes the backplane window selector or depends on a window that one moved. Two knobs
mean the census can be re-flown at any time — to re-confirm an identity cross-check on a machine, or
to isolate a regression — without arming a single write.

| Place | Entry |
| --- | --- |
| `unaos/crates/kernel/Cargo.toml` | `wifi2 = ["wifi"]` |
| `unaos/crates/kernel/src/wifi/mod.rs` | `#[cfg(feature = "wifi2")] pub mod bringup;` + the one call, from `finish_and_park` — reached only on a terminal `stage_attempt` outcome, never on `Pending`/`Retry` |
| `unaos/arroyo` (feature mapping) | `UNAOS_WIFI2=1` → `wifi2` **alone** — the Cargo implication pulls `wifi` in, and pushing both would put a duplicate in the comma list the `arm_features` strip rewrites textually |
| `unaos/arroyo` (`arm_features`) | stripped for aarch64, same argument as `wifi`'s |
| `unaos/arroyo` (`KERNEL_CFG_MATRIX`) | appended to the `x86-all` leg |
| `unaos/builder/src/main.rs` | `UNAOS_WIFI2` → `wifi2` |

The builder wiring is not optional, for the reason §4 gives and this arc inherits doubled: a
bring-up that silently did not run is indistinguishable on the wire from a radio that would not
answer — and that is precisely the conclusion the ladder's next decision would then rest on.

**Where arc 2 runs, and the one boot it never reaches.** `bringup_once()` is called from
`finish_and_park`, the shared terminal of `S_WAIT_STORAGE` and `S_WAIT_ALT`, reached only on a
`Settled` outcome or a `Retry` budget exhausted — so `staged_count()` is final before the
completeness gate reads it. Neither a `Retry` (count still moving) nor WIFI-REACH's `Pending` (a
second volume may still arrive) reaches arc 2. The consequence, stated rather than discovered later:
a boot where the program-source block device never appears waits in `S_WAIT_STORAGE` and never
reaches arc 2 at all; a boot where the program source is short and a second handle never appears
waits in `S_WAIT_ALT` up to `ALT_WAIT_MS` and then commits its INCOMPLETE. Since WIFI-REARM/REACH
those waits are visible (the heartbeats above) rather than silent, and the alternative — walking the
backplane on a timer while storage is still enumerating — is still refused, because it would put a
window write in a race with the very pass that decides whether an upload may follow.

Arc 1's own wiring, unchanged:

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
| **1 (landed, FLOWN)** | Config-space identification of the radio, cross-checked against pinned metal facts + locate/validate/stage the firmware set from the program-source volume | **No.** Config-space reads and FAT reads only. |
| **2 (landed)** | Map BAR0, walk the bcma core table from our own reads, identify the d11 core and its wrapper, read the core + wrapper state, re-measure §S3's enable rule | **Config `0x80`, always** (moved and restored). **A backplane write only on the branch where the core does NOT arrive enabled**, and only its reversible half. |
| 3 | §S4's reset-to-known-state prologue + the microcode upload, then PHY/radio init from the staged initvals, a receive path, a scan, one authenticate/associate exchange, bound to `smolnet` through the existing `net_phy` seam | Yes — including the first destructive one |

Arc 2's mechanics are `bcm4331.md` §S1b/§S1c/§S3, not this document. **The upload moved out of arc 2
and into arc 3**, for the reason in "Where arc 2 stops" below; the ladder table above records that
rather than leaving the original plan standing.

## Arc 2 — what the code does

`unaos/crates/kernel/src/wifi/bringup.rs`, witnesses prefixed `wifi2:`.

| rung | device access | reversible? |
| --- | --- | --- |
| R0 precondition | PCI config READS — identity re-read LIVE, PMCSR, COMMAND, BAR0 | n/a |
| R1 map | `map_mmio_window(bar0, 0x2000)` + `translate()`. A page-table edit; **the device sees nothing** | n/a |
| R2 pre-image + unwind self-test | writes cfg:0x80 with `pre_win \| 0xFFF`, reads back, restores `pre_win` — a **discriminating** round-trip over bits the selector ignores | n/a |
| R3 ChipCommon | cfg:0x80 ← `0x18000000`, readback **enforced**, then MMIO reads (`chipid`, `erom`) | yes — R8 |
| R4 EROM walk | cfg:0x80 ← the EROM page, readback **enforced**, then MMIO reads | yes — R8 |
| R5 d11 window | cfg:0x80 ← the d11 base, readback enforced; core + wrapper reads. cfg:0xac written **only if** firmware left it off this core's master wrapper | yes — R8 |
| R6 enable rule | **no write when the core arrives enabled**; else a witnessed `RESET_CTL`/`IOCTL` pair, **unwound in place** if the enable does not take | yes |
| R7 upload | **REFUSED** at a named UNKNOWN; a dry-run line describes the stream it would push | n/a |
| R8 restore | cfg:0x80 ← the RECORDED pre-image, MATCH-verified against that value | — |

R3–R7 live in one `explore()` whose every exit is a plain return, and R8 runs unconditionally on the
way out — so the restore is guaranteed by control flow rather than by a reader checking that each
refusal remembered to fall through.

Every device write carries a **pre-read / write / post-read witness triple**, and every count on the
`end` line is a FIELD of the `Writes` struct incremented at its write site — including the audited
zeros. `wrote-core-regs=` and `uploaded-bytes=` were string literals in the first cut, which is an
audited zero that audits nothing: the day something writes a core register, a literal keeps saying
zero. They are values now, and `wrote-cfg80=` / `wrote-cfg0xac=` / `wrote-wrapper=` are broken out by
purpose (`selftest`/`moves`/`restore`, `enable`/`unwind`) so a total is never misread as "the window
moved five times".

### The restore discipline, and why it is not a formality

The pre-image of `cfg:0x80` on this machine is **`0x18001000`** — the d11 core — not the enumeration
base. A stage that "restored" to `0x18000000` because that is what it assumed firmware left would
silently move the radio's window for every later boot stage AND look clean in every log line. R2
records the value it reads, R8 pushes that value, and the MATCH is computed against it. `0x18000000`
appears nowhere as a restore target.

R2 also proves the unwind BEFORE the window moves — and the proof has to be **discriminating**. The
first cut wrote the pre-image back to itself and checked the readback, which a config-write path that
drops every write passes trivially, because the register still holds the pre-image. A self-test that
cannot fail is the instrument defect this tree keeps finding in its own gates.

The probe is `pre_win | 0xFFF`. The selector is 4 KiB-granular, so this value selects the *same*
backplane page — the window provably cannot move — while being a *different* 32-bit word from the one
the register holds. Then `pre_win` is pushed back and verified. Two outcomes are honest and are
reported apart: `discriminating=1` when the readback is the probe (the write path demonstrably took),
and `discriminating=0` when the readback is `pre_win` (this silicon masks the low 12 bits, so the
probe could not distinguish took-from-dropped, and no other value we may write leaves the window
unmoved). Anything else, or a final readback that is not the pre-image, is a hard refusal with the
window still on the firmware value.

`took=` is likewise **enforced** at R3, R4 and R5, not merely printed. The first cut checked it only
at R5 while printing it at R3 and R4 — an internal inconsistency on the file's most important
invariant, and a reader could not tell which of the three lines was load-bearing.

### The EROM walk is TAG-driven, and that is the design

`bcm4331.md` §S1b records three defects that made the first walk in this tree find zero cores. The
third was fixed arity: the walker consumed exactly `nsp + nmw + nsw` address descriptors, but an EROM
declares **ports**, not descriptors, and a port's list ends only when the next entry stops matching.

Arc 2 avoids the whole class differently rather than transcribing the fix. After a component's
identifier pair it consumes **every** master-port and address entry the tags identify, in order,
until the next entry is a component identifier or the end sentinel. Synchronisation therefore does
not depend on the declared counts at all — which frees those counts to be what they should be: an
**independent cross-check**, printed as `declared(nmp=… nsp=… nmw=… nsw=…)` against
`observed(mp=… slave=… swrap=… mwrap=…)` with an `arity=MATCH|MISMATCH` verdict on every component
line. A wrong CIB count mask now shows up as a MISMATCH on the wire instead of as a silent desync.

### The cross-check that makes the walk a measurement

The `d11 FOUND` line compares the walked core against four metal boots (`rev=29`,
`base=0x18001000`, `mwrap=0x18101000`) **and** against the two config registers Apple's firmware left
behind (`cfg:0x80` and `cfg:0xac` pre-images). Those are entirely independent sources — a register
firmware wrote before we existed, and an on-chip ROM we walked ourselves. Agreement to the bit is
what separates a measurement from a plausible-looking decode.

**Four conditions gate the core window, and two of them were computed-and-ignored in the first cut.**
`rev_ok` and `base_ok` were always checked. `mwrap_ok` was printed and discarded — so a d11 whose
master wrapper disagreed with four metal boots still got `cfg:0xac` pointed at that address and *both*
R6 writes issued through the resulting aperture; the one MISMATCH leading directly to a backplane
write was the one nothing acted on. `arity_ok` was printed per component and never consulted — but an
arity MISMATCH means the cursor's idea of where a component ended may be wrong, so `base`/`mwrap` may
have been read out of a NEIGHBOURING component's descriptors, which is precisely the address the
cross-check exists to keep out of the selector. All four now gate, with the failing one named
(`rev-mismatch` / `base-mismatch` / `mwrap-mismatch` / `arity-mismatch`).

Reading `d11+0x120` on the wrong core reads another register entirely, and writing that core's
wrapper writes another core's reset control — so a number printed from there is worse than no number,
and a write issued there is worse still.

Two upstream gates protect the same property. `chipid != 0x13924331` now REFUSES before `BAR0+0xFC`
is treated as an EROM pointer: the first cut printed that MISMATCH, discarded it, and wrote the
derived address into the selector — pointing the window at an address handed over by a register it
could not identify. And the walk's structural bound is computed per-EROM as
`(0x1000 - erom_off) / 4` rather than a fixed 1024: with a non-zero offset the fixed cap let the
cursor read past the core window's edge into the **wrapper aperture** at `BAR0+0x1000` and decode
agent registers as EROM entries.

### Master wrapper, not slave wrapper

`bcm4331.md` §S1c records a deliverable line that once advised `cfg:0xac <- 0x00000000`, because it
took the wrapper address from `swrap` and this core declares `nsw=0 nmw=1`. Arc 2 carries `mwrap` and
`swrap` separately, uses the master wrapper, refuses when there is none, and prints both.

### The enable rule is re-measured, never assumed

§S3 concluded that reachability costs zero writes **on this part as it arrives**. That is a
measurement, and arc 2 re-takes it every boot: `(IOCTL & (CLK|FGC)) == CLK` and `RESET_CTL.RESET`
clear, each printed with its operands. `reachability=SATISFIED(no-write)` is the expected reading and
`enable-writes-made=0` is the audited consequence.

`reachability=REQUIRED` is the alarm — a cold power cycle that skips the EFI AirPort init, a
different card, or a firmware revision that leaves the radio down. On that branch, and only there,
arc 2 deasserts `RESET_CTL.RESET` and sets `IOCTL.CLK` with `FGC` clear, leaving every other IOCTL bit
exactly as read. The rule is then re-evaluated on the POST-write readbacks rather than on what we
intended to write.

**And if the enable does not take, both writes are UNWOUND in place.** The first cut printed
"reversible: IOCTL <- …" and returned, leaving the wrapper half-written with the undo instructions on
the wire for a human to perform. A write is not reversible because its pre-image was printed; it is
reversible because something reverses it. The unwind restores `IOCTL` first and `RESET_CTL` second —
the reverse of the write order, so the core is not re-held in reset while its clock configuration is
still ours — each with its own pre/post triple, and the refusal line carries `unwind-verified=`.

Re-asserting `RESET_CTL.RESET` there is **restoration, not the forbidden direction**: that bit was
found SET on this branch, firmware itself left it that way, and there is no resident microcode to
destroy because the core arrived in reset — which is why the branch ran at all. §S4's prologue is a
different act: it asserts reset on a *running* core specifically to clear `PSM_RUN`. Arc 2 never does
that.

### Where arc 2 stops — a NAMED UNKNOWN, not an omission

Two gates stand between arc 2 and the upload, and both are on the wire.

**Gate 1 — the set.** Boot A staged 0/3. The upload obviously cannot run; the point worth stating is
that **§S4's prologue is skipped with it**. That prologue asserts `RESET_CTL.RESET`, which clears
`PSM_RUN` and destroys the resident microcode, and `bcm4331.md` §5 risk 4 records that *only a
successful upload restores a working state*. A destructive, unrecoverable write whose sole
justification is an upload that cannot happen is not made.

**Gate 2 — the routing selector, and it stands even with the set present.** §S4 describes the upload
as `SHM_CONTROL <- (B43_SHM_UCODE << 16) | 0` followed by a stream into `SHM_DATA`. **The numeric
value of `B43_SHM_UCODE` appears in no source this module may use.** §S4 names the symbol, not the
value; this tree carries only `SHM_ROUTE_SHARED = 0x0001` (the READ routing §S4a uses); no capture of
ours has measured it. The one source that carries it is b43 driver source, which
`CLEAN_ROOM_POLICY.md` §2 puts off-limits for `src/wifi/`.

Guessing it is not a small risk dressed as a large one, and the asymmetry is §S4a's own: that probe's
safety argument is that it **never writes the data port**, so a wrong routing costs a wrong number on
a witness line. An upload inverts that exactly — a wrong routing streams ~90 KB into whatever bank the
window actually selected. So R7 refuses at the selector, names the unknown, and says what would
settle it: the value from a source legal for this module, or a metal probe that identifies the
routing read-only.

When the set IS complete, R7 still prints a **dry-run** line first — `hdr=`, `type=`, `ver=`,
`declared=`, the payload offset, the payload byte count and its be32 word count. That line is what
settles W3 (below) and is what arc 3 starts from. It makes no device access.

### Open UNKNOWNs after arc 2

| # | Unknown | Probe |
| --- | --- | --- |
| W3 | The firmware container header layout — §S4's own record is internally inconsistent | Unchanged from arc 1, and still open because Boot A staged 0/3. Now readable from TWO lines on the first boot with the real set: arc 1's `STAGED … hdr=` and arc 2's `upload PREPARED(dry-run) … hdr= payload-offset=`. They share one `classify_header`, so they cannot disagree |
| W5 | The `B43_SHM_UCODE` routing value | Not settleable by any boot of this code. See gate 2 |

W4 — "does the tag-driven walk agree with the declared arities?" — is not listed as an unknown
because it is not one the code waits on: every component line answers it with `arity=`.

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
:: wifi: firmware staging deferred — no program-source block device yet (the set lives on that FAT volume); re-checking every pass, staging re-arms whenever it appears ::
:: wifi: still deferred n=1/6 waited=10000ms — no program-source volume yet (handles=global=absent sdhc=unbuilt); the poll is LIVE ::
:: wifi: program-source volume APPEARED after waited=17431ms (handles=global=present sdhc=unbuilt) — resuming firmware staging at attempt n=1/8 ::
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

Firmware staging — WIFI-REARM's deferral forms (**non-terminal**: nothing was staged, the attempt is
re-armed):

```
:: wifi: staging attempt DEFERRED at mount — program-source volume present but would not mount (block I/O error); nothing staged, re-attempting ::
:: wifi: staging attempt DEFERRED at root-dir — mounted source=global … but the root directory did not read (block I/O error); nothing staged, re-attempting ::
:: wifi: staging re-armed — attempt n=3/8 deferred at stage=mount, next attempt in 1000ms ::
:: wifi: firmware NOT staged — 8 attempts all deferred at stage=mount against a present handle (handles=global=present sdhc=unbuilt); giving up staging for this boot ::
```

Only the last of those four is terminal. `DEFERRED` and `NOT staged` are deliberately different
words: the first says "not yet", the second is this boot's answer.

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
* **Clean, no volume at all (Boot D's shape):** the census lines through `cross-check=PASS`, the
  `staging deferred` line, then up to six `still deferred n=k/6` heartbeats and silence. That silence
  is now the DOCUMENTED tail of a live poll rather than an unexplained one, and an arrival after it
  still produces the `volume APPEARED` line and a staging verdict.
* **Failure:** any `MISMATCH`, `cross-check=FAIL`, `REFUSED`, `REJECTED`, or `NOT staged` line, each
  carrying its own reason. A boot that prints no `wifi:` lines at all means the knob was not armed —
  check the `⚡ kernel features:` banner for `wifi`, and check `builder/src/main.rs` if the media was
  produced by the builder rather than by `arroyo`.

---

## Arc 2 witness lines (`wifi2:`)

The expected shape on the bench rMBP, in order. Every device write appears as a pre/post triple, and
the pass ends in exactly one `end` line whatever happened.

```
:: wifi2: begin — arc 2: map BAR0, walk the EROM from our own reads, … WRITES: cfg:0x80 … ::
:: wifi2: precond 04:00.0 vendor=0x14e4 device=0x4331 d-state=0 mem-decode=1 bus-master=1 cmd=0x0006 bar0=0x00000000c1900000 kind=mem64 — LIVE config re-read, not the census's cached copy ::
:: wifi2: map bar0=0xc1900000 len=0x2000 translate=ok pa=… — two 4 KiB apertures … ::
:: wifi2: pre-image cfg:0x80=0x18001000 cfg:0xac=0x18101000 — the 0x80 value is the ONE restore target ::
:: wifi2: unwind-selftest PASS discriminating=1 probe=0x18001fff probe-readback=0x18001fff restore-readback=0x18001000 — the probe differs from the pre-image only in the low 12 bits the selector ignores, so the window provably did NOT move, and the readback proves the write path took ::
:: wifi2: WROTE cfg:0x80 pre=0x18001000 new=0x18000000 readback=0x18000000 took=1 — window now on ChipCommon ::
:: wifi2: cc-raw chipid=0x13924331 expected=0x13924331 MATCH erom=0x18107000 expected=0x18107000 MATCH ::
:: wifi2: cc-decode id[15:0]=0x4331 rev[19:16]=2 pkg[23:20]=9 nrcores[27:24]=3(SB-era CoreCount — ADVISORY…) type[31:28]=1 (bcma/erom) is-4331=1 ::
:: wifi2: WROTE cfg:0x80 pre=0x18000000 new=0x18107000 readback=0x18107000 took=1 — window now on the EROM … ::
:: wifi2: erom-core[0] id=0x800 (chipcommon) mfg=0x4bf(bcm) rev=37 class=0 base=0x18000000 … arity=MATCH ::
:: wifi2: erom-core[1] id=0x812 (802.11(d11)) mfg=0x4bf(bcm) rev=29 class=0 base=0x18001000 mwrap=0x18101000 swrap=0x0 bridge=0 declared(nmp=1 nsp=1 nmw=1 nsw=0) observed(mp=1 slave=1 swrap=0 mwrap=1) arity=MATCH ::
:: wifi2: erom-walk components=N entries=E arity-mismatches=0 stop=end-tag verdict=WALK-OK elapsed=…ms ::
:: wifi2: d11 FOUND id=0x812 rev=29 expected=29 MATCH base=0x18001000 expected=0x18001000 MATCH mwrap=0x18101000 expected=0x18101000 MATCH swrap=0x00000000 arity=MATCH ::
:: wifi2: d11 cross-check base-vs-cfg0x80-preimage=MATCH (…) mwrap-vs-cfg0xac-preimage=MATCH (…) — a config register FIRMWARE wrote versus an on-chip ROM WE walked ::
:: wifi2: WROTE cfg:0x80 new=0x18001000 readback=0x18001000 took=1 — window now on the RADIO core ::
:: wifi2: core-pre macctl=0xc0020403 psm-run=1 psm-jmp0=0 shm-enabled=1 shm-upper=0 big-endian=0 tsf=…:… ::
:: wifi2: core-pre phy-ver raw=0x9701 analog=9 type=7 expected=7 MATCH rev=1 — type 7 is the HT-PHY … ::
:: wifi2: wrapper aperture cfg:0xac=0x18101000 == the EROM's master wrapper 0x18101000 — firmware's own value, NOT written by us ::
:: wifi2: wrapper-pre ioctl=0x00002055 iost=0x0000100c resetctl=0x00000000 resetst=0x00000000 ::
:: wifi2: enable-rule (ioctl&(CLK|FGC))=0x0001 want=0x0001 match=1 (resetctl&RESET)=0x0 clear=1 => core-enabled=1 ::
:: wifi2: reachability=SATISFIED(no-write) enable-writes-made=0 — … Every read above was taken WITHOUT them, which is the corroboration ::
:: wifi2: upload SKIPPED reason=firmware-set-incomplete staged=0/3 — and the §S4 PROLOGUE is skipped with it … ::
:: wifi2: RESTORE cfg:0x80 <- pre-image 0x18001000 readback=0x18001000 restored=MATCH ::
:: wifi2: end ok=1 stage=d11 d11=FOUND wrote-cfg80=6(selftest=2 moves=3 restore=1) wrote-cfg0xac=0(moves=0 restore=0) wrote-wrapper=0(enable=0 unwind=0) wrote-core-regs=0(audited — SHM_CONTROL, SHM_DATA, MACCTL and RADIO_CONTROL share this counter …) uploaded-bytes=0(audited) restore=MATCH elapsed=…ms ::
```

With the set present, the two `upload SKIPPED` / `RESTORE` lines are preceded by:

```
:: wifi2: upload PREPARED(dry-run, no device access) ucode bytes=… hdr=A|B|unrecognized type=… ver=… declared=… payload-offset=… payload-bytes=… be32-words=… words-whole=… — … W3 … is settled by the hdr= field on this line ::
:: wifi2: upload REFUSED reason=shm-ucode-routing-UNPINNED — … NOT guessed. What settles it: … ::
:: wifi2: upload NOT ATTEMPTED uploaded-bytes=0(audited) wrote-core-regs=0(audited — SHM_CONTROL, SHM_DATA, MACCTL and RADIO_CONTROL share this counter and no site in this file increments it) — the resident microcode is untouched and still running ::
```

Refusal forms, each naming its own reason and each leaving the window where it found it:

```
:: wifi2: REFUSED stage=census reason=no-radio-selected … — NOTHING written … ::
:: wifi2: REFUSED stage=census reason=cross-check-failed … arc 2 is GATED on that check passing … ::
:: wifi2: REFUSED stage=identity reason=not-4331 … ::
:: wifi2: REFUSED stage=precond reason=not-d0|mem-decode-off … ::
:: wifi2: REFUSED stage=bar0 reason=bar0-unassigned|bar0-is-io … ::
:: wifi2: REFUSED stage=map reason=bar0-unmapped … ::
:: wifi2: REFUSED stage=unwind-selftest reason=readback-mismatch … the window has NOT been moved off the firmware pre-image ::
:: wifi2: REFUTED window-hypothesis — … BAR0+0 reads 0xffffffff … the fault is upstream of the selector ::
:: wifi2: erom-pointer UNUSABLE erom=… ::
:: wifi2: d11 ABSENT — no component id=0x812 with a slave address … must not be guessed ::
:: wifi2: d11 REFUSED reason=rev-mismatch|base-mismatch|mwrap-mismatch|arity-mismatch — the core window is NOT opened and cfg:0xac is NOT pointed at this component … ::
:: wifi2: REFUSED stage=chipcommon reason=selector-did-not-take readback=… want=0x18000000 ::
:: wifi2: REFUSED stage=chipcommon reason=chipid-mismatch chipid=… expected=0x13924331 id[15:0]=… rev[19:16]=… pkg[23:20]=… type[31:28]=… — BAR0+0xFC is NOT known to be an EROM pointer and NO address derived from it is written into the selector ::
:: wifi2: REFUSED stage=erom reason=selector-did-not-take readback=… want=… ::
:: wifi2: REFUSED stage=selector reason=did-not-take ::
:: wifi2: REFUSED stage=core reason=core-window-dark macctl=0xffffffff ::
:: wifi2: REFUSED stage=wrapper reason=no-master-wrapper|cfg0xac-did-not-take ::
:: wifi2: reachability=REQUIRED — the core did NOT arrive enabled (§S3's alarm) … ::
:: wifi2: enable-write 1/2 RESET_CTL pre=… wrote=… post=… took=… settle=… — unwind target: RESET_CTL <- … ::
:: wifi2: enable-write 2/2 IOCTL pre=… wrote=… post=… took=… settle=… — unwind target: IOCTL <- … ::
:: wifi2: enable-verify (ioctl&(CLK|FGC))=… (resetctl&RESET)=… => core-enabled=… verdict=ENABLED|STILL-DOWN ::
:: wifi2: enable-unwind 1/2 IOCTL pre=… wrote=… post=… restored=MATCH|FAILED settle=… ::                       (STILL-DOWN only)
:: wifi2: enable-unwind 2/2 RESET_CTL pre=… wrote=… post=… restored=MATCH|FAILED settle=… — re-asserting a bit FIRMWARE had asserted is RESTORATION, not the §S4 prologue … ::
:: wifi2: REFUSED stage=enable reason=still-down — the wrapper has been UNWOUND to the words it was found with (ioctl=… resetctl=…), unwind-verified=MATCH|FAILED ::
```

A `settle=` value is printed with its **unit**: `us` when the bootpace TSC rate is calibrated, `cy`
when it is not. A microsecond claimed off an uncalibrated counter is a fabricated number, so the code
does not make one.

## Provenance ledger — arc 2

**What was adopted.** `bringup.rs`'s constant block (~59 register offsets, bit masks and EROM entry
encodings) and its `Erom` cursor scaffolding (`struct`, `new`, `stopped`, `peek`, `take`, `ci`,
`at_end`, `master_port`) are **adopted from `drivers/bcma.rs`** and inherit that module's recorded
Group-B provenance — its own header states that its "register offsets and EROM encodings follow Linux
`drivers/bcma` (`bcma_regs.h`, `bcma_driver_chipcommon.h`, `scan.c`) and `b43`'s `B43_MMIO_PHY_VER`".
What is independent is the parse strategy alone: `Erom::address` and the `walk_erom` driver consume a
component's descriptors by tag until the next identifier, rather than by declared port count.

The implementer of `bringup.rs` read no GPL Linux WiFi driver source directly; every externally
derived fact arrived through `bcm4331.md` and `drivers/bcma.rs`. That describes the route, not the
origin, and does not change the classification.

| class | meaning | facts carried under it |
| --- | --- | --- |
| `[METAL]` | measured by THIS kernel on OUR bench | `0x18000000` answers as ChipCommon (Boot AJ); `chipid=0x13924331`; `erom=0x18107000`; d11 `base=0x18001000`, `mwrap=0x18101000`, `rev=29` (Boots AL/AM/AN, three identical walks); PCI identity/BAR0/subsys (Boots AF/AJ + arc 1's own Boot A) |
| `[PUBLIC]` | public PCI base spec / PCI-SIG registry | config-header layout, capability-list walk, PM capability + PMCSR D-state encoding, COMMAND bit 1, BAR type bits, Broadcom `0x14E4` |
| `[LEDGER]` | externally derived **and** corroborated by a capture of ours that **could have come out otherwise** | cfg `0x80`/`0xAC` as the window selectors (Boot AJ wrote one and the chip started answering — a discriminating outcome); `BCMA_CORE_SIZE=0x1000` and the 2×4 KiB BAR0 extent; every EROM entry encoding (§S1b, anchored to our own first EROM word `0x4BF80001`, whose alternative decode is structurally impossible, and corroborated by three full walks landing on an address firmware independently wrote); core ids `0x800`/`0x812`; manufacturer `0x4BF`; `TSF 0x180/0x184` (the pair ADVANCED between samples); `PHY_VER 0x3E0` (`0x9701` decodes to analog 9 / type 7 / rev 1 — structured, and exactly what an HT-PHY 4331 should say) |
| `[EXT-CORROBORATION-WEAK]` | externally derived, and the capture usually cited is **non-discriminating** — the register answered with *something*, but no reading available to us would have differed had the offset or bit been wrong. **Only discriminating source: b43.** | wrapper offsets `IOCTL 0x408`, `IOST 0x500`, `RESET_CTL 0x800`, `RESET_ST 0x804`; `IOCTL_CLK`, `IOCTL_FGC`, `RESET_CTL_RESET`; `MACCTL 0x120`; `MACCTL.PSM_RUN`/`SHM_ENABLED`/`SHM_UPPER`/`BE` |
| `[EXT-UNPINNED]` | externally derived, not corroborated at all | `MACCTL.PSM_JMP0 = 0x4` — decoded only, never written |

**The consequence, stated plainly: R6 — the one rung that writes a backplane register — is gated
entirely on `[EXT-CORROBORATION-WEAK]` facts.** The enable rule reads `IOCTL` and `RESET_CTL` at
adopted offsets and tests adopted bit positions, and on the not-enabled branch it writes both. The
previous version of this table called those seven constants `[LEDGER]` and cited
`ioctl=0x00002055 iost=0x0000100c resetctl=0 resetst=0` as corroboration — but four arbitrary offsets
inside a live 4 KiB agent window also return non-all-ones values, and `resetctl=0`/`resetst=0` are
exactly what a wrong offset landing on a reserved word returns. That citation proved nothing and has
been withdrawn. Combined with the adoption above, the honest position is that arc 2's only backplane
write rests on b43-derived facts, which is why R6 now unwinds both writes in place on the STILL-DOWN
path rather than describing them as reversible and returning.

The one fact §S4 does **not** carry at all — the `B43_SHM_UCODE` routing value — is the arc's W5
above, and is why R7 refuses.

## Gates — arc 2

* `./arroyo check` — green, both arches, 12 cfg legs (`wifi2` appended to the `x86-all` leg).
* `UNAOS_WC=1 ./arroyo check` — green.
* `UNAOS_WIFI=1 ./arroyo check` — green.
* `UNAOS_WIFI=1 UNAOS_WIFI2=1 ./arroyo check` — green.
* **Reachability, not just compilation.** `UNAOS_WIFI=1 UNAOS_WIFI2=1 ./arroyo esp-x86` then `strings`
  over `target/x86_64_esp/kernel.elf` finds **50** `wifi2:` witness strings in the release LTO image,
  so the rungs survived dead-code elimination and are reached from the metal knob line. With
  `UNAOS_WIFI=1` alone the same search finds **0**, and arc 1's own 20 `:: wifi: ` strings are present
  in both. The banner reads `…,wifi,wifi2` armed and `…,wifi` disarmed. The count rose from 46 to 50
  with the review fixes, and the four added strings are the ones that matter most: the two
  `enable-unwind` triples, `REFUSED stage=enable reason=still-down`, and
  `REFUSED stage=chipcommon reason=chipid-mismatch`.
* **QEMU cannot reach this path.** It models no BCM4331, so arc 1's census refuses and arc 2 never
  runs; a green QEMU boot is vacuous here and is not claimed as a gate.

**One measured non-property, recorded rather than glossed.** The arc-1-only image is *not*
byte-identical to the pre-arc-2 build: `UNAOS_WIFI=1 ./arroyo esp-x86` gives `kernel.elf` sha
`8d0bac3e…` on this tree and `b4cb40da…` on the same tree with `wifi/mod.rs`, `wifi/firmware.rs` and
`Cargo.toml` reverted to the parent commit (both builds forced, and the build was confirmed
reproducible under a forced recompile of identical sources).

The delta was **measured, not assumed benign**. Sorting the two images' string sets and diffing gives
944 differing lines, and **every one of them contains `.llvm.`** — LLVM internal symbol-name hashes,
which rehash on any edit anywhere in the crate. Zero non-`.llvm.` strings differ; not one `wifi`
string differs; the arc-1 witness count is 20 in both. The additions that made the difference are
themselves `#[cfg(feature = "wifi2")]`-gated and compile to nothing in this build.

The property that matters and does hold is the aarch64 one: `wifi2` is stripped by `arm_features`, so
no Pi or Jetson media hash moves.

For the record, the three x86 images this comparison rests on:

| build | `kernel.elf` sha256 | banner |
| --- | --- | --- |
| parent commit, `UNAOS_WIFI=1` | `b4cb40da58ea98d3…` | `…,wifi` |
| this arc, `UNAOS_WIFI=1` | `8d0bac3efb294aa6…` | `…,wifi` |
| this arc, `UNAOS_WIFI=1 UNAOS_WIFI2=1` | `bcd6e1e06b3ccfbf…` | `…,wifi,wifi2` |

---

## Gates — WIFI-REARM

Base sha `3bc0ead0`. Lane: `unaos/crates/kernel/src/wifi/` (`mod.rs`, `firmware.rs`) and this
document. No storage driver, video, bt or gen7 file is touched.

* `./arroyo check` — green, both arches, 12 cfg legs.
* `UNAOS_WC=1 ./arroyo check` — green.
* `UNAOS_WIFI=1 ./arroyo check` — green.
* `UNAOS_WIFI=1 UNAOS_WIFI2=1 ./arroyo check` — green.
* **Knob-off byte-identity — VERIFIED, not asserted.** `mod.rs` claims that with the knob off "every
  image is byte-identical to baseline", and that is the one identity claim this arc could have
  broken. `./arroyo esp-x86` gives `kernel.elf` sha
  `ac5d175981fab7e73cb242828a3e98418d242effaba2829f3e051b6c40bcfed8` on this arc and the SAME sha on
  the same tree with both wifi files reverted to `3bc0ead0` (measured by snapshotting the diff to
  `~/unaos-bench/scratch/wifi-rearm/`, `git apply -R`, building, and re-applying — never `git stash`).
  The build was confirmed reproducible first: a forced rebuild of the unchanged knob-off tree
  reproduced that sha exactly.
* **Reachability, not just compilation.** `strings` over the release-LTO
  `target/x86_64_esp/kernel.elf`: `:: wifi: ` counts **26** with `UNAOS_WIFI=1` (20 before this arc —
  the six added are the heartbeat, the arrival line, the re-arm line, the two staging-deferral
  lines and the give-up line), and `wifi2:` counts **0** with `UNAOS_WIFI=1` alone and **50** with
  both knobs — unchanged from arc 2, so `wifi2` still emits nothing into an arc-1-only image. The
  arc-1 count is **26 in both** the `wifi` and the `wifi,wifi2` image. Banners read `…,wifi` and
  `…,wifi,wifi2`.
* **QEMU cannot reach this path** and is not claimed as a gate: QEMU models no BCM4331, so the census
  refuses before the wait ever arms.

The arc-1 image is expected NOT to be byte-identical to `3bc0ead0`'s — six new witness strings are
the point of the arc. For the record:

| build | `kernel.elf` sha256 | banner |
| --- | --- | --- |
| `3bc0ead0`, knob off | `ac5d175981fab7e7…` | (no `wifi`) |
| this arc, knob off | `ac5d175981fab7e7…` | (no `wifi`) |
| `3bc0ead0`, `UNAOS_WIFI=1` | `8bdfff03ccaea25e…` | `…,wifi` |
| this arc, `UNAOS_WIFI=1` | `9320cc14bc7e22b7…` | `…,wifi` |
| this arc, `UNAOS_WIFI=1 UNAOS_WIFI2=1` | `8b3f7e02f3199157…` | `…,wifi,wifi2` |

One measured detail, recorded because it would otherwise look like a discrepancy in a later review:
the armed shas move on a **comment-only** edit inside the module (intermediate builds of this arc
gave `2b1d844323541fdb…` and `56f5f04bd1d1d4ae…`; the review round below moved them again). That is
the `.llvm.` internal-symbol rehash the arc-2 gate section already measured, not a codegen change;
the arc-1 witness count (26) and the banner were identical across every one of them. The knob-off
sha did not move across ANY of them — including the review round — which is the property that
matters here.

### Review round

The adversarial review re-ran all four `check` legs, re-measured the four shas above from scratch
(all four of the arc's original values reproduced exactly, including `3bc0ead0`'s armed
`8bdfff03ccaea25e…` and its 20-line `:: wifi: ` count), and re-verified the knob-off identity by the
snapshot/`git apply -R`/rebuild route. Two defects were fixed, neither behavioural on the wire's
happy path:

1. **The arrival line printed once per attempt, not once per arrival.** It read
   `retry n=k/8 — … volume APPEARED after waited=Xms`, and on an unsettled transport a capture would
   carry up to eight of them with eight different `waited=` values — a reader counting arrivals
   would have counted eight. It is now gated on `n == 1`, reworded to lead with the event, and the
   attempt counter it used to carry is read off the `staging re-armed` lines, which already carry
   `n=`. The string count is unchanged (26): the same format string, reordered.
2. **The arc-2 precondition was documented as `Settled`-only, and the code does not enforce that.**
   The exhausted-budget arm falls through to `bringup_once()`. The behaviour is right — an exhausted
   budget IS a terminal answer, `staged_count()` is final at 0 because both `Retry` arms stage
   nothing, and holding arc 2 back there would have *removed* the arc-2 witness from a boot where
   `stage_once()` used to produce it — but the comment at the call site, this document's "what did
   NOT change" paragraph, and the outcome table all claimed a stricter gate than the code has. All
   three now state the real precondition: arc 2 runs on a TERMINAL staging answer, `Settled` or
   budget-exhausted, and is held back only by the non-terminal `Retry`.

Independently confirmed during the review, against the tree rather than the prose: all three
`wifi::service()` call sites (`main.rs:1103`, `:1534`, `:4241`) sit inside persistent `loop {}`
bodies, so the arc's central premise — that Boot D's poll really was live through those nine silent
minutes — holds; the heartbeat is emitted from inside the `program_source().is_none()` arm and so
cannot print unless that poll actually ran on that pass (it is a witness that can fail); the quiet
branch past `MAX_SPOKEN_WAITS` stores no latch that touches the poll; the counter arithmetic yields
exactly six heartbeats and `arch::ticks()` is a non-wrapping u64 ms counter; both `Retry` arms are
above the `FW_SET` loop, so no retry can double-stage or resume a half-built set; and `bringup.rs`
is untouched by the arc, so the `B43_SHM_UCODE` upload refusal is unchanged by construction.

**Metal proof is Boot B**, and its falsifiable prediction is the chain in the WIFI-REARM section
above: on a repeat of Boot D's no-storage shape the capture must carry six `still deferred n=k/6`
heartbeats where Boot D carried nothing, and on a boot whose volume arrives the `volume APPEARED` line must
be followed by a staging verdict and the arc-2 chain.
