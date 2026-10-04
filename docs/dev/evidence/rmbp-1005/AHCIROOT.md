# AHCIROOT — the rMBP's SSD becomes the UnaFS root (B332; R82)

Branch `exec-rmbp-ahciroot`, cut from 8c750d43 (merge11 tip). Knob **`UNAOS_AHCIROOT=1`** (Cargo
`ahciroot = ["ahci-write", "unafs", "installdemo"]`; x86 only, a k8-reach NA row like its parents).

## Design (written before the code)

**Finding.** Most of the SATA write ladder already exists and is unreachable from a mount: AHCIWRITE
(B89) put `WRITE DMA EXT` behind `ahci-write` and a `WriteGrant` value minted only in
`install::partition` (`grant_range`, the one `WriteGrant::new` call), and SELFINSTALL2 (B310) lays
ESP + a sector-cloned UnaFS through `amber_core::Plan`. What is missing: (1) the PLAIN port path
`block::write_block_ahci_port` — the one UnaFS and FAT reach — refuses unconditionally, so the SSD's
UnaFS can never be the read-write `/`; (2) no `FLUSH CACHE EXT`, no readback, the error register printed
raw; (3) a card with no UnaFS gets an ESP-only SSD; no scratch region exists for a write proof;
(4) six hand-rolled GPT / protective-MBR readers in the kernel beside `amber_core`'s; (5) `bind_mount`
never admits an AHCI handle.

**Seam.** `Kernel — driver` for the grant and the write (the block layer is the fulfiller);
`Amber Bytes — shared-core` for every GPT read (`amber_core::gpt`, new `read_table_with`,
`classify_sector0`, `protective_slot`, `has_signature`); `fs-core` for the volume (`unafs::UnaFS::format`
is `tools/unafs init`'s own code — the crate is already the shared no_std core, nothing is copied).

**The grant (M1).** ONE kernel-held slot (`block::AHCI_LIVE_GRANT`) holding a `WriteGrant` (port +
inclusive LBA range) and its kind (`install` / `root` / `test`). It is filled only from a `WriteGrant`
that `install::partition` minted (the audit grep is unchanged: one `WriteGrant::new`), returned as an
RAII `GrantHold` that empties the slot — and flushes the drive — on drop. `install ssd --write` holds it
for the verb's lifetime only after `selfinstall::probe` + `selfguard` judged the disk (blank/ours, or
the operator typed the confirmation token the refusal printed: `--erase-stranger <token>`). A boot root
grant is minted (silently) only when an AHCI disk is judged OURS (every partition a UnaOS ESP or UnaFS,
no foreign type GUID, the ESP carries BOOTX64.EFI + kernel.elf) and covers p2 only; it lives for the
boot. `write_block_ahci_port` with no live grant for that port/LBA returns `BlockError::Denied` (-EPERM)
and prints one `:: [ahci] write -EPERM …` line (rate-limited).

**Milestones.**
- M1 GRANT — slot, hold, `Denied`, the refusal line; `install ssd --write` holds it; a live root
  grant makes `--write` refuse (`reason=ssd-is-live-root`).
- M2 WRITE — `FLUSH CACHE EXT` (0xEA, non-data: PRDTL 0, 10 s bound); the port path writes through
  `write_sectors_granted`, reads every sector back and compares, flushes; ATA error register decoded
  (ICRC/UNC/IDNF/ABRT/AMNF) on failure.
- M3 LAYOUT — the plan's p2 is the volume + 64 scratch sectors at its tail; a card with no UnaFS gets a
  fresh p2 formatted by `UnaFS::format` through the held grant; the six GPT readers move onto amber_core.
- M4 ROOT — `bind_mount` tries the root-granted SSD first (SSD root over card root); `unafs_state`
  answers `present`; one line `:: UNAFSX86: root=unafs src=ahci:<port> blocks=<n> gen=<g> home=/home
  boot=<card> -> PASS ::`; `/boot` stays the card's ESP.
- M5 `tests ahciw`.

**Witness.** `:: AHCIROOT: grant=<none|port N> write=<refused|ok> readback=<eq|ne> flush=<ok|fail> -> PASS|FAIL|SKIP ::`
(plus the M4 root line at boot; nothing else prints at boot, R80).

**Owed.** The SSD's ESP is not bound as `/boot` (the card's stays `/boot`; booting the SSD alone binds
its own ESP as before). p2 is not grown to the disk (fresh p2 = 4 GiB). No multi-sector PRDT (one
sector per command). The operator-confirmation path on the GLASS (instgui) is not wired; the shell
token is. Unflown (R78).

## Results (unflown, R78)

Commits: M2 d35847d4 · M1 e0c4f5b1 · M3 ecf71433 · M4+M5 f60e83d1 (M3 and M4+M5 were finished from the
previous executor's uncommitted tree after a harness interrupt; only the tip is compile-proven).

- `cargo test -p amber_core` (repo root): exit 0, 3 tests, KAT 44 checks.
- x86 metal shape + `ahciroot`: exit 0. x86 metal shape without `ahciroot`: exit 0.
- aarch64 login shape (`login,loginst,virt_el0`, blob head 280080d2): exit 0.
- charter-check: exit 0.

Expected wire on a metal boot of an installed SSD (knob `UNAOS_AHCIROOT=1`):
`:: UNAFSX86: root=unafs src=ahci:<port> blocks=<n> gen=<g> home=/home boot=<card> -> PASS ::`, and on
`tests ahciw`: `:: AHCIROOT: grant=port <N> <a>..<b> kind=root (lba 0 outside) write=refused errno=-EPERM lba=0 … -> PASS ::`
then `:: AHCIROOT: grant=port <N> write=ok readback=eq flush=ok scratch=<s>..<e> lba=<s> restored=ok -> PASS ::`.
An SSD laid before AHCIROOT reads `-> SKIP (… reinstall)` on the second line (its volume reaches the tail).
