# Zenbook ledger — every finding, ticked once

> Arch ledger for the ASUS Zenbook UM5606W track (AMD Ryzen AI 9 HX 370 "Strix Point", x86_64; Peter 2026-10-27:
> "begin support for the um5606w hardware"). Cross-arch and shared-seam items live in `docs/dev/LEDGER.md` (ids `S<n>`,
> `P<n>`, and the seat-prefixed `SO`/`SP`/`SR`); the rMBP's x86 findings live in `docs/dev/OS/rmbp-ledger.md` (A, B, E).
> Ids here are `Z<n>` — orin uses A-D, rmbp A/B/E, pi `PI<n>`, so a distinct prefix keeps the files mergeable without
> renumbering. The queue (the ORDER) is `docs/dev/OS/zenbook-queue.md`; the hardware sheet is
> `docs/dev/OS/01_BOOT_HAL/um5606w.md`.

> Created 2026-10-27 by ZENBOOKSURVEY (wave 36, cloud seat), with Z1. The `owner` column carries `rmbp` until
> `unaos/scripts/ledger-check.sh` learns a `zenbook` owner (its owner set is {orin, pi, rmbp, shared-gate}; the queue's
> ZENBOOK-LEDGERGATE row): the x86 seat carries this track and the code is the x86 kernel. Nothing has booted on the
> Zenbook; the first boot is the bench's.

## Findings

| id | item | owner | flies-on | status | evidence | closed by |
|---|---|---|---|---|---|---|
| Z1 | ZENBOOKSURVEY — the x86 kernel's Mac assumptions measured and the UM5606W track opened: the loader hands the SMBIOS entry point (`BootInfo.smbios_ptr`, tagged `SMBIOS_TAG`), `hw::platform()` reads the Type 1 product and prints `[hw] platform= dmi=` once at `pci.rs init`; the SMC, gmux and HDA probes ask `hw::gate` and print `[hw] skip <probe> platform=` on a positively identified non-Apple machine (the rMBP, QEMU, any Apple machine and a machine with no readable SMBIOS run today's code); the Strix Point PCI ids name their census rows (`hw::pci_id_name`); queue, ledger and hardware sheet created | rmbp | QEMU x86 lanes (`tests platform`); the Zenbook bench's first boot | fixed-unflown | docs/dev/OS/01_BOOT_HAL/um5606w.md; `tests platform` | — |
