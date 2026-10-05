# ENSEMBLE — the installer is a handler ensemble

Peter, 2026-10-01 (RULINGS **R79**): *"numerous handlers are involved in the installer, no?"* The
installer is not one program. It is several handlers, each one owning its own domain, and it ships
as the `UnaOS_Installer` vessel (**R31**). The kernel's `install/` engine is **one fulfiller** of the
disk leg (Ring 0, over the same shared core). It is not a second implementation (audit **B296**,
R28). This file lists the members, the verbs each one answers, and the order an install calls them
in. Status words: **live** (on the bus or CLI today, tested), **core** (the shared `no_std` logic
exists, but no bus surface yet), **owed**.

## 1. Members

| Member | Domain in the install | Code | Status |
|---|---|---|---|
| **Amber Bytes** (The Block) | disks: enumerate, plan the layout, lay the GPT, format FAT32 / UnaFS, verify, recover, clone | `handlers/amber_bytes` (bus + `amber` CLI) over `unaos/libs/sys/amber_core` and `unaos/libs/fs/unafs` (`unafs::format`) | **live** (AMBER1, SR34): DiskList, PlanLayout, Apply, Verify, Recover. Clone: **core** (`amber_core::ClonePlan`; the kernel runs it, the host verb is owed) |
| **Geode** (The Vault) | the image: the payload the install copies (UnaFS image, ESP tree), its archive format and digest | `handlers/geode` | **owed** (installer leg). The kernel's `src extract` gunzip/tar/sha256 is the stand-in (B296) |
| **Principia** (The Architect) | the install's settings: target disk choice, hostname, locale, boot policy; the `install` limits SELFDIAG reads | `handlers/principia`, `unaos/libs/sys/prefs_core` | **core** (prefs). Install keys **owed** |
| **Holocron** (The Key) | the first user's credentials, the plan-signing key (HMAC `Signer`, §3) | `handlers/holocron` | **owed** (installer leg) |
| **Vein** (The Mind) | SELFDIAG (B324): read a failed boot's witness lines, propose a fix, rebuild, reboot | `handlers/vein` | **owed** (R82, after LUMENAPP) |
| **Comscan** (IO Bridge) | the serial wire in companion mode (a second machine drives the install) | `handlers/comscan` | **owed** (installer leg) |
| **Midden** (The CLI) | the verbs an operator types (`install ...`, `amber ...`) and their help | `unaos/libs/sys/midden_core` | **live** (verb table shared). Install verbs are kernel-side today |
| **Helm** (The Wheel) | the human consent on a destructive physical action: the second key when an AI drives the install | `handlers/helm`, `unaos/libs/sys/helm` | **owed** (installer leg) |

## 2. Amber Bytes verbs (live)

JSON on the wire, one request and one response (`handlers/amber_bytes/src/bus.rs`). The `amber` CLI
runs each command as the same verb, through `Amber::handle`. `amber bus` serves the wire over stdio.

| Verb | Topic | Request | Response | Writes |
|---|---|---|---|---|
| `DiskList` | `amber/disk/list` | none | `Disks{disks:[{name,dev,sectors,logical_block_size,removable,read_only,virtual_dev,model,vendor,partitions,mounts}]}` | never (reads sysfs + the mount table; opens no device node) |
| `PlanLayout` | `amber/plan/layout` | `{layout, target?, disk_sectors?}` | `Planned{disk_sectors, layout, dry_run, scheme, signature}` | never |
| `Apply` | `amber/apply` | `{target, layout, scheme, signature, dry_run}` | `Applied{written, lines}` | yes, unless `dry_run` |
| `Verify` | `amber/verify` | `{target}` | `Verified{ok, worst, lines}` | never (a UnaFS probe mounts through a write-capturing recorder) |
| `Recover` | `amber/recover` | `{target, dry_run}` | `Recovered{direction, written, lines}` | yes, unless `dry_run` |
| any failure | — | — | `Error{message}` | — |

**A layout** is text (`amber-layout v1`, then `disk-seed`, then one `part <kind> <size> <name>
[seed=] [format=fat32|unafs|none] [label=] [spc=]` line per partition). It does not depend on disk
size: the plan is the layout laid out on the medium in hand.

**The signed-plan rule.** `PlanLayout` returns the exact dry run and a signature over its canonical
bytes. Those bytes are: the table's every sector write with its CRC-32, every FAT32 format write at
its absolute LBA, and each UnaFS volume's parameters. `Apply` derives the plan again for the target
it opens. It refuses unless the signature verifies over *those* bytes. So a changed layout, a
different disk size, or a different seed cannot be applied under an old signature. The scheme today
is `sha256`, an unkeyed digest. It proves the caller saw this plan, not who the caller is.
`trait Signer` is the seam for HMAC-SHA-256 under a Holocron key.

**The write policy** belongs to the handler. A bus caller cannot widen it. A regular file (an image)
may always be written. A block device needs `--yes-i-mean-it` **and** an allowlist entry (`--allow`
or `AMBER_ALLOW`), and it is refused while it or any of its partitions is mounted. The decision is
taken on the canonical path.

**After every write, the result is read back.** `Apply` reads the table back through the core's
whole-table verify, which must match the plan. It reads back every byte-carrying FAT32 write, and it
mounts and fscks every UnaFS volume. `Recover` re-verifies after it writes.

## 3. An install, in order

1. **Principia** answers which disk and which layout (owed; today the operator chooses).
2. **Amber Bytes** `DiskList`: the candidates. A mounted disk is flagged and cannot be written.
3. **Amber Bytes** `PlanLayout` on the target: the dry run and its signature are shown to the
   operator (or to **Helm**, when an AI drives).
4. **Holocron** signs the shown plan under the install key (owed; the `sha256` digest stands in).
5. **Amber Bytes** `Apply` with that signature: the table and formats are laid, then read back.
6. **Geode** supplies the payload (the UnaFS image, the ESP tree). **Amber Bytes** clones it into the
   planned partitions (owed on the host; the kernel's `install ssd --write` does this leg today over
   `amber_core::ClonePlan`).
7. **Amber Bytes** `Verify`: the table and every filesystem are probed.
8. **Vein** reads the first boot's witness lines (SELFDIAG, owed).

## 4. Fulfillers

The disk leg has two fulfillers over one core. One is the host handler (`handlers/amber_bytes`, this
file's verbs). The other is the kernel's `install/` engine, which links `amber_core` by path with
`default-features = false`. Both write the same bytes for the same plan. The golden card KAT pins
this (`amber_core::kat`). `tools/una-card` writes it too, and its table is checked byte for byte
against the `amber plan` dry run (`tools/una-card/tests/card_table.rs`). The kernel's six
hand-rolled GPT readers are moving onto the core (AHCIROOT's kernel sibling).
