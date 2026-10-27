# Zenbook ledger — every finding, ticked once

> Arch ledger for the ASUS Zenbook S 14 OLED UM5606W track (AMD Ryzen AI 9 HX 370 "Strix Point", Radeon 890M,
> MediaTek MT7925, NVMe, USB4; its own panel is dead, so it runs on an external monitor). Cross-arch and
> shared-seam items live in `docs/dev/LEDGER.md` (ids `S<n>`, `P<n>`, and the seat-prefixed `SO`/`SP`/`SR`);
> rows below cross reference it by id. Ids here are `Z<n>` — orin's arch ledger uses A-D, rmbp's A, B, E and
> pi's PI, so a distinct prefix keeps the files mergeable without renumbering.

> Created 2026-10-27 by ZENBOOKNVME (wave 36) because the file was absent at its cut (281b09c3e);
> ZENBOOKSURVEY creates the same file in flight beside it, and the seat unions the two at the fold.
> Peter, 2026-10-27: "begin support for the um5606w hardware". NO METAL until Peter says: the first boot is
> the bench's.

Status: **open** · **fixed-unflown** (in tree, not on metal) · **flown** (scored on the wire) ·
**landed** · **dropped** (ruled out, with the ruling).

## Z. Zenbook lane

| id | item | owner | flies-on | status | evidence | closed by |
|---|---|---|---|---|---|---|
| Z2 | **ZENBOOKNVME — the NVMe host controller, rung 1.** `drivers/nvme.rs` (NEW, CHARTER Amber Bytes / driver, `nvme` knob, x86_64): probe by class 01/08/02, 64-bit BAR0, reset (CC.EN=0) and enable bounded by CAP.TO, admin queue depth 8, IDENTIFY controller and namespace 1, ONE I/O queue pair depth 32, READ/WRITE with PRP1/PRP2/one PRP list page through a 32 KiB bounce, completions polled on the phase bit against a 1000 ms bound (`[nvme] cmd timeout opcode=` marks the controller), the masked bounded hold `with_ctrl`; publishes into `block::register_nvme` (block.rs tail, AHCI first rung's free-function registry shape); `[nvme] ctrl mn="<m>" ns=1 lba=<n> blocks=<n> -> ready`. `tests nvme`: sector 0 through `block::read_blocks_nvme`, and on QEMU's controller only a write + read-back + restore of 8 scratch blocks: `:: NVME: ctrl=ok ns=1 rw=ok -> PASS ::`. Knob five-place (arroyo, builder feats + `-device nvme,drive=nvme0,serial=una` on a fresh target/nvmefixture.img, Cargo.toml, k8-reach.registry, banner-cert.sh). OWED (storage.md §5): NVMEBOOT (handle + source + root grant), NVMEWRITE, MSI-X, multiple namespaces, TRIM, the SMART/Health log page, NVMe on the Orin, BOUNDEDWAIT, DISKHOLD. | rmbp (cloud wave 36) | QEMU `UNAOS_NVME=1` then the Zenbook bench | fixed-unflown | code-only, uncompiled at commit; the seat's x86all leg compiles it; the bench runs `tests nvme` | — |
