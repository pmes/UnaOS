# Landing report — INSTALL-CORE (the storage-agnostic installer engine)

Branch: `us-installcore` (off `main` 088a06a). Knob: `UNAOS_INSTALLDEMO=1` (feature `installdemo`),
default OFF. Date: 2026-07-18.

## What landed

The RUNG-3 ENGINE of the installer line, built and proven on a scratch block device before it is ever
pointed at a real card: a **GPT writer + FAT32 formatter + extent-level copy-and-verify**, expressed
over one small abstraction — the `InstallTarget` trait — so the same engine will later drive the Orin
microSD / Pi emmc2 / an x86 USB stick unchanged.

New module `crates/kernel/src/install/`:
- `mod.rs` — `InstallTarget` trait, `BlockTarget` (over `drivers::block`), `InstallError`,
  `blank_check` (the never-touch guard), the copy-verify orchestration, and the witness `run_demo` /
  one-shot `install_probe_once`.
- `gpt.rs` — protective MBR + primary/backup GPT (UEFI CRC-32/ISO-HDLC), ESP + data partition entries,
  and `verify_gpt` (parse-back self-verify, part of the write API).
- `fat32.rs` — FAT32 formatter (BPB/FSInfo/FATs/root) + the extent-recording payload writer.
- `hash.rs` — self-contained no_std CRC-32/ISO-HDLC + SHA-256 (KAT-gated at witness time).

Wiring (all knob-gated, kept in sync): `crates/kernel/Cargo.toml` (`installdemo` feature),
`arroyo` + `builder/src/main.rs` (map the knob to the feature), and `builder/src/main.rs` attaches a
fresh all-zero 128 MiB scratch image (`target/installscratch.img`) over the usb-storage slot under the
knob (the boot ESP is a separate `ide-hd`, never touched). x86 witness call site added in
`main.rs`'s main loop (`install_probe_once`, one-shot, gated on storage). Doc:
`unaos/docs/dev/OS/10_INSTALL/installer_engine.md` (NEW section 10_INSTALL).

## Design choices of record

- **GPT layout** mirrors the host-side `builder/src/vm_image.rs`: 512-B sectors, protective MBR @0,
  primary header @1 + array @2..33, ESP @LBA 2048 (1 MiB aligned) ≤ 64 MiB, a Microsoft-Basic-Data
  partition filling the rest, backup array/header at the tail. Two partition entries so the platform
  boot layout (bootable ESP + data area) exists from the first write. Deterministic GUIDs.
- **FAT32**: 512 B/sector, 1 sector/cluster, 32 reserved, 2 FATs, standard `fatgen` FAT-size. The
  formatter writes only the *defining* structures and relies on the blank-precondition (the guard
  enforces an all-zero target) for the guaranteed-zero FAT/data remainder — the "do it right" note is
  in the doc. The produced volume is mounted + read back by the IN-TREE FAT reader as an interop check.
- **Verify by content**: GPT write self-verifies (re-read + re-CRC); the copy primitive records exact
  extents and SHA-256-checks by re-reading them; a 1-byte-corruption negative test proves the verifier
  rejects, then restores + re-verifies.
- **Arch-neutrality**: the engine drives only `drivers::block`, so it COMPILES on both arches under
  `installdemo` (pub API → no dead-code warning on aarch64); the witness is x86-only (the QEMU scratch
  disk is x86 this arc). Stated per the brief.
- **Scratch = usb-storage slot** (not a genuinely-second drive): the block layer is single-device; a
  second usb-storage would need xHCI multi-device support (out of lane). Reusing the slot keeps the
  boot ESP cleanly separate.

## Refusal-path evidence (never-touch discipline)

- Pre-write: `blank-check (pre-write) => BLANK, armed` — the armed scratch is confirmed blank before
  any write.
- Post-write: `blank-check (post-write) => NOT blank, engine would REFUSE => guard OK` — after a GPT
  is present, the same guard refuses, so the engine will not clobber an occupied volume.
- `blank_check` performs NO writes on refusal (returns `InstallError::NotBlank`).

## DONE gate — results verbatim

- `./arroyo check` (both arches, knob OFF): x86_64 OK, aarch64 OK.
- `UNAOS_INSTALLDEMO=1 ./arroyo check` (both arches, knob ON): both `Finished` (engine compiles on
  aarch64 too).
- `UNAOS_INSTALLDEMO=1 ./arroyo test 22`: the 11-line INSTALL witness incl. the negative test, ending
  `:: INSTALL: gpt+fat32+copy verify => PASS ::`. FAIL scan (awk) = 0.
- knob-OFF `./arroyo test 22`: 0 INSTALL lines (module absent), FAIL scan = 0, unregressed.
- `UNAOS_CPU=qemu64 ./arroyo test 22` (knob OFF, xAPIC fallback): FAIL scan = 0.
- `./arroyo test-arm 22`: 0 UnaOS `FAIL` verdicts (the only lowercase "fail" hits are baseline OVMF
  firmware boot chatter — "Image start failed", TPM2, BdsDxe, ConvertPages).
- `./arroyo kernel8-test 35`: 0 `FAIL` verdicts; full K-series + BANDY battery ran (47 PASS/anchor
  lines), unregressed.

## Flagged

- **Serial burst line-drop (infra quirk, worked around, not fixed).** At the first prints after the
  block device enumerates, the serial console reliably drops the SECOND of two back-to-back, pre-I/O
  writes (confirmed: a sentinel first line survived, the following line dropped; every later INSTALL
  line, each spaced by real block I/O, survives). The witness folds its two opening prints into one
  line to stay robust. This is pre-existing serial infrastructure (outside this lane); worth a look if
  other witnesses ever lose an opening line.
- Payloads are bounded to a single-FAT-sector chain (≤ ~125 clusters) — ample for this arc's few-KiB
  payload; a larger file would need multi-FAT-sector RMW (guarded with `BadArg`), a later rung.
