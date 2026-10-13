# FLIGHT 35 — installing UnaOS onto the rMBP's internal SSD, beside macOS

Peter, 2026-10-14: *"is installing unaos onto the rmbp hard drive coming soon"*. This is the
procedure for that sitting. It is written against the tree INSTALL4 (rmbp-ledger **B619**) left; the
seam-by-seam audit behind every line is `docs/dev/evidence/install4/AUDIT.md`.

**The sitting is ATTENDED and it is Peter's.** Every write below is typed by Peter. Nothing writes the
SSD at boot (INSTALL4 M1), and every write needs the administrator's session (R100), a grant the
installer mints only after its refusals pass, and a token typed for THIS disk (R25: a stranger's disk
changes only by the operator's explicit act).

**What this flight does:** census the SSD, print the plan, put an ESP and a UnaFS partition in FREE
space beside macOS, and boot UnaOS from the SSD's own loader. **What it never does:** write LBA 0,
write any sector of an existing partition, erase macOS, or set a firmware boot variable (R29: the
loader has no runtime services; R3: the ⌥ picker is the human step).

Serial lines are read with `awk 'index($0,"<tag>")'` on the capture (never bare `grep`). Every
`<…>` is a value the machine prints; the shapes are the code's own format strings.

---

## 0. Preconditions — the seat's, before the card is cut

1. The image is built from `unaos/scripts/image-line.env` at a tree carrying INSTALL4 (`set -a;
   . unaos/scripts/image-line.env; set +a; ./arroyo esp-x86`). The line carries
   `UNAOS_INSTALLDEMO=1`, `UNAOS_AHCI_WRITE=1`, `UNAOS_AHCIROOT=1`, `UNAOS_LOGIN=1` and NOT
   `UNAOS_INSTGUI` (the window would open at every boot, R86 — this flight types the verb).
2. The build's `⚡ kernel features:` banner names `installdemo`, `ahci-write` and `ahciroot`, and
   `scripts/banner-cert.sh` certifies the three on the artifact (rows `:: INSTALLVERB: census disk=`,
   `:: AHCI: write path ARMED`, `:: AHCIROOT: grant=`).
3. `./arroyo test-install` is green at that tree, including its five INSTALL4 checks
   (`INSTALL4 ssd-blank`, `boot-blank`, `ssd-beside`, `beside-image`, `boot-beside` each `-> PASS`).
   **If any of the five is red, the flight stops after §3 (the dry run): the write path has not been
   watched to work on any disk.**
4. Peter's first user exists (it is the administrator, R100). macOS is up to date with its own
   backups; UnaOS never writes macOS's partitions, but a backup is the operator's to keep.

## 1. Boot the card (as on every flight)

Card in the SD slot, power on holding ⌥, pick the card's "EFI Boot". Log in as the administrator.

Read on the wire (once each):

```
:: install: boot volume serial=0x<card ESP serial> — INSTALL-SELF boot-device guard ARMED ::
:: AHCI: port=0 model="APPLE SSD SM768E" sectors=1467339812 lba48=1 ::     (shape from flight 8)
:: UNAFSX86: root=unafs disk=sdhc blocks=<n> gen=<n> home=/home -> mounted ::
```

**Write down the boot volume serial.** §5 compares it.
If the image printed `[install] armed, verbs only: …` — that is M1's line: the boot fixture wrote
nothing. **ABORT A0:** any `:: INSTALL: fixture start`, `:: INSTALL: grant minted` or
`[ahci] grant held` line at boot, before Peter typed anything, is a defect: power off and report.

## 2. The census — read-only

Type in the shell:

```
install
```

Read, for the SSD (`disk=ahci0`):

```
:: [midden] cmd="install" -> Host verb=install ::
:: INSTALLVERB: census disk=ahci0 transport=ahci ::
:: INSTALL: census disk=<id> parts=<n> foreign=<f> friend=0 empty=<e> ::
:: INSTALL: census part=0 type=c12a7328 lba=<a>..<b> sectors=<n> content=ESP ::
:: INSTALL: census part=1 type=7c3457ef lba=<a>..<b> sectors=<n> content=APFS ::
```

Check these against what macOS's Disk Utility says about the same disk (SITTING-1 §1.2, still the
rule): the partition COUNT matches `parts=`; **the Catalina container reads `content=APFS` AND
`type=7c3457ef`**. **ABORT A1:** if the APFS container reads `unknown`, `empty` or any other type —
the probe is wrong about the one volume it exists to protect. Stop and report; nothing has been
written.

## 3. The dry run — read-only

```
install ssd --dry-run
```

Read:

```
:: [midden] cmd="install ssd --dry-run" -> Host verb=install ::
[install] target=ahci:0 model=<model> sectors=<n> gpt=valid verdict=stranger plan=<plan>
[install] saw: foreign=<f> friend=0 [part0:ESP,part1:APFS…] — a stranger's OS lives here
```

`verdict=stranger` is the right answer for a disk carrying macOS. **ABORT A2:** any other verdict on
this disk — stop and report.

Then ONE of two blocks:

**(a) there is room** — `plan=beside(free=<a>..<b> <MiB> MiB: esp 512 MiB + unafs <MiB> MiB in empty
slots <i>,<j>; the <n> existing partitions untouched) then --write --beside BESIDE-0-<sectors>`, and:

```
[install] beside free=<a>..<b> (<MiB> MiB) slots=<i>,<j> existing=<n> — LBA 0 and every existing partition stay outside every write
[install] plan: 2 parts on <sectors> sectors (<MiB> MiB)
[install]   p1 esp   lba <a>..<b> 512 MiB UNAOS-ESP
[install]   p2 unafs lba <a>..<b> <MiB> MiB UNAOS-UNAFS
[install] beside token=BESIDE-0-<sectors>
:: INSTALL4: beside port=0 free=<a>..<b> esp=512 unafs=<MiB> slots=<i>,<j> existing=<n> -> DRY ::
```

Check: `free=` starts AFTER the last macOS partition's `lba=…<b>` in §2 (the free space is where
macOS left it); `existing=` equals §2's `parts=`. Go to §4.

**(b) there is no room** — `plan=REFUSE(stranger: no-free-space (have=<MiB> MiB need=<MiB> MiB; …) —
never a wipe)` and `:: INSTALL4: beside port=0 reason=no-free-space -> REFUSED ::`. Go to §3a.

The other refusals stop the flight here (nothing was written; report the line):
`gpt-not-standard` (**ABORT A3**: macOS's two copies of the table disagree, or an array is not where a
standard table keeps it), `no-free-slot`, `beside-present` (a UnaFS partition is already on the disk).

### 3a. Making room — in macOS, Peter's act (only after (b))

UnaOS never shrinks a stranger's volume. Boot macOS (⌥ → Macintosh HD) and give back free space AFTER
the APFS container with macOS's own tool — at least the `need=` the dry run printed; more is fine
(the UnaFS partition takes the whole free extent and grows its volume to it):

```
diskutil list                                   # the APFS container is disk0s2 on this machine — check
sudo diskutil apfs resizeContainer disk0s2 <smaller size, e.g. 400g>
```

(macOS's tool and its output; not UnaOS's wire. Local Time Machine snapshots can refuse a shrink —
`tmutil listlocalsnapshots /` shows them.) Then repeat §1–§3: the dry run must now print block (a).

## 4. The write — beside macOS

Only with §0.3 green, block (a) printed, and the administrator logged in. Type the token exactly as
the dry run printed it (letter case does not matter):

```
install ssd --write --beside BESIDE-0-<sectors>
```

Read, in this order:

```
[install] beside target=ahci:0 model=<model> sectors=<n> gpt=valid verdict=stranger saw=<…>
[install] plan: 2 parts on <sectors> sectors (<MiB> MiB)
[install] snapshot src=sdhc files=<n> bytes=<n>
:: INSTALL: grant minted transport=ahci port=0 BESIDE lba=<esp first>..<unafs last> — free space only, every in-use partition outside it ::
:: INSTALL: grant minted transport=ahci port=0 BESIDE lba=<unafs first>..<unafs last> — free space only, every in-use partition outside it ::
:: INSTALL: grant minted transport=ahci port=0 GPT lba=1..<n> and lba=<n>..<last> — the two tables only, LBA 0 and every partition outside ::
[ahci] grant held port=0 kind=install lba=<unafs first>..<unafs last>
:: AHCI: write path ARMED — opcode WRITE-DMA-EXT-0x35 (ATA8-ACS, LBA48) compiled and reachable only through a WriteGrant (first, once) ::
[install] beside table=ok slots=<i>,<j> esp=<a>..<b> unafs=<a>..<b> (both headers, both arrays, CRCs, re-read)
[install] file=<path> bytes=<n> ok=1 sha=<hex>                  (one per ESP file)
[install] unafs mirror: <clone plan> src=sdhc                   (then progress, then `unafs mirror done`)
[install] beside grow from=<blocks> to=<blocks> ok
:: INSTALL4: beside port=0 table=ok old=<n>/<n> untouched=<m>/<m> esp=512 unafs=<MiB> cloned=<MiB> files=<n> verified=<n> fsck=ok ms=<ms> -> PASS ::
:: SELFINSTALL: files=<n> bytes=<n> ms=<ms> verified=<n> -> PASS ::
[ahci] grant dropped port=0 kind=install flush=ok
```

`untouched=m/m` is LBA 0 plus the first 4 KiB of every partition that was on the disk before, hashed
before the first write and again after the last: **it is the line that says macOS was not touched.**
The mirror copies the card's UnaFS volume sector by sector at the SATA write rate — minutes.

The console's last line: `install ssd --write --beside: <n> files verified <n>/<n>, UnaFS fsck=ok, the
<n> existing partitions byte-identical — reboot holding Option and pick EFI Boot`.

**ABORT A4** — any `:: INSTALL4: beside REFUSED reason=<token> — nothing written ::` (boot-device,
grant-slot-held, token-mismatch, not-a-stranger, source-on-target, snapshot-failed, grant-refused, …):
nothing was written. Read the reason; report it.
**ABORT A5** — `:: INSTALL4: beside port=0 table=fail … -> FAIL ::`: the table edit did not verify and
the table as it was read was written back. Stop. Boot macOS and confirm Disk Utility shows the disk as
before.
**ABORT A6** — `table=ok` then any `-> FAIL` (`esp=fail`, `fsck=fail`, `verified` short,
`untouched` short): the two NEW partitions are not trustworthy. Do not boot them. Read `untouched=`
first: if it is short, report it at once. Recovery §7.

## 5. The first boot off the SSD — card IN

Shut down. **Leave the card in** (the card-less boot is owed: §8). Power on holding ⌥. The picker
should show Macintosh HD, the card's "EFI Boot" and a second "EFI Boot" for the SSD's ESP (whether this
firmware lists a second EFI partition on the internal disk is a firmware fact no wire has carried yet —
this boot measures it). Pick the INTERNAL disk's EFI Boot (the internal-disk icon, not the SD card's).

Read:

```
:: install: boot volume serial=0x<serial> — INSTALL-SELF boot-device guard ARMED ::
:: UNAFSX86: root=unafs src=ahci:0 blocks=<n> gen=<n> home=/home boot=sdhc -> PASS ::
```

`src=ahci:0` is `/` on the SSD's UnaFS. `boot=sdhc` is expected with the card in: the kernel finds
itself BY CONTENT and the card carries the same kernel the SSD's loader started; on this disk the
Mac's own EFI partition is the first FAT volume, and today's FAT layer offers bootdisk only that one
(AUDIT row 12). Which loader ran is the picker choice plus the serial: if §1's serial differs from this
one, the SSD's loader ran. If they are equal (both ESPs carry amber_core's serial, AUDIT row 15), the
picker choice is the only witness — say so in the flight record.

**ABORT A7:** the picker lists no second EFI Boot — record it (it is the firmware fact this boot
exists to measure). The install is on the disk; `bless` from macOS is the operator's option (R3,
R29), measured at the bench, not given here.

Then, read-only: `install ssd --dry-run` → `verdict=stranger … plan=REFUSE(stranger: beside-present …
— never a wipe)` (the SSD now carries our UnaFS beside macOS).

**After this install, a card boot on this image also roots `/` on the SSD** (AHCIROOT's design:
`:: UNAFSX86: root=unafs src=ahci:0 … boot=sdhc`). The card's own UnaFS root comes back only by
removing the UnaOS partitions (§7) or a build with `UNAOS_ROOT_PREFER=sdhc`.

## 6. Abort points at a glance

| id | where | line | what it means | do |
|---|---|---|---|---|
| A0 | boot | any `:: INSTALL: fixture start` / `grant minted` / `grant held` before a typed command | the unattended fixture ran on media | power off, report |
| A1 | §2 | the Catalina container not `content=APFS type=7c3457ef` | the census is wrong about macOS | stop, report |
| A2 | §3 | `verdict=` not `stranger` | the classifier is wrong about this disk | stop, report |
| A3 | §3 | `reason=gpt-not-standard` (also no-free-slot, beside-present) | nothing to write beside | stop, report |
| — | §3 | `reason=no-free-space` | macOS has the whole disk | §3a, or stop |
| A4 | §4 | `:: INSTALL4: beside REFUSED reason=…` | a refusal; nothing written | read, report |
| A5 | §4 | `table=fail … -> FAIL` | the table was written back as read | stop; check in macOS |
| A6 | §4 | `table=ok` then `-> FAIL` | new partitions untrustworthy | §7; `untouched=` first |
| A7 | §5 | no second EFI Boot in the picker | firmware fact | record |

## 7. Recovery

- **The card still boots.** The installer only READS the card (the snapshot and the UnaFS mirror are
  reads); boot it with ⌥ as on every flight.
- **macOS's own partitions are untouched** — `untouched=m/m` measured it at the write, and no grant
  this tree mints covers LBA 0 or a sector any pre-existing partition owns. ⌥ → Macintosh HD.
- **Removing UnaOS from the SSD** (Peter's act, in macOS): `diskutil list` shows `UNAOS-ESP` and
  `UNAOS-UNAFS` by name; `sudo diskutil eraseVolume free none <each one>` returns them to free space;
  `sudo diskutil apfs resizeContainer disk0s2 0` gives it back to macOS. (macOS's tools; not UnaOS's
  wire.) After that a card boot roots on the card again.
- **A failed install that left a UnaFS superblock** may still be taken as `/` by a card boot (AHCIROOT
  roots on any one UNAFS-typed UnaFS partition): if that boot misbehaves, remove the partitions as above.

## 8. What stays refused (this flight)

- `install ssd --write --erase-stranger <token>` — it lays a new table over the WHOLE disk and erases
  macOS. It is an explicit operator act R25 permits, and it needs Peter's own fresh word; it is not part
  of this procedure.
- `install <disk> <slot>` (INSTALLVERB) onto the SSD — it writes a synthetic test tree, not this system.
- The card-less boot off the SSD — owed: `fs/fat.rs` mounts only a GPT disk's FIRST FAT volume (the
  Mac's EFI), so the kernel cannot find itself on the SSD's ESP without the card.
- Any `BootOrder`/`BootNext` write (R29) — the startup disk is ⌥ (R3) or `bless` from macOS.
- The installer window (`UNAOS_INSTGUI`) — it opens at every boot (R86); and its stranger screen offers
  only the whole-disk erase (a beside screen is owed).
