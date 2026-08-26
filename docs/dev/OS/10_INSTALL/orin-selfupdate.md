# ORIN-SELFUP — the OS writes its own boot files (Jetson Orin Nano)

Status: implemented behind `UNAOS_SELFUP=1` (feature `selfup`), QEMU-unreachable (the flow is
tegra-metal by construction), unflown on the bench. Code: `unaos/crates/kernel/src/arch/aarch64/
selfup_tegra.rs`; call site: `main.rs::tegra_early_stop`, directly after the ORIN-INSTALL-2 site;
payload builder: `./arroyo selfup-pak`.

## 1. Shape

The required update shape (baton orin-6 §5.2): **receive bytes → sha256 verify → write boot volume →
warm reboot**, under the BOOTABI **matched-pair rule**: the loader and the kernel are a matched
pair — an update writes the WHOLE ESP or nothing, never a fresh kernel beside an old bootloader.

The core (verify → write → reboot) is **permanent and transport-agnostic**. The first byte-source is
scaffolding-tolerant by design brief: a staged payload already present on the boot volume. The seam
between the two is exactly two file names (§3).

## 2. Transport survey and the pick

In cost order (the brief's order — whatever works today without new defect-fixing first):

| transport | cost today | verdict |
|---|---|---|
| **staged payload on the boot volume** | zero new kernel code on the receive side — the FAT *read* path has been the boot path since JB2b, and the bench card loop already moves files onto the volume | **PICKED** — v1 byte-source |
| serial (UARTC, XMODEM-shape) | a framing protocol + hours-scale transfer at 115200 for a ~15 MiB ESP; and the serial line is dev scaffolding — LAWS forbid building anything permanent on it | rejected |
| LAN TCP | the RX ring one-pass latch defect (NET-4F) is being worked by a peer executor; building on it now = building on a known defect | deferred — the intended endgame; lands by delivering the same two staged names (§3), core unchanged |
| USB re-plug (new stick) | works, but it is not self-update — it is the existing manual media path | not this arc |

The pick keeps every permanent line transport-blind: a future TCP receiver's whole job is to make
`UPDATE.PAK` + `UPDATE.SHA` exist in the boot-volume root and (optionally) re-invoke the service.

## 3. The transport seam

Two files in the boot volume root:

* `UPDATE.PAK` — the UPK1 container (§4) holding the complete new ESP tree.
* `UPDATE.SHA` — sha256 hex of `UPDATE.PAK` (`sha256sum` line shape; hash first, rest ignored — the
  same shape SOURCE-ALONG's `SRC.SHA` already uses).

Built on the host by `./arroyo selfup-pak [src_esp_dir] [out_dir]` (defaults: the `esp-jetson`
output tree, `target/`). The builder enforces the matched-pair rule at build time too, so a partial
payload is refused on the host before it ever reaches a card.

## 4. UPK1 container

Little-endian throughout; all sizes `u32` (a FAT file size is a `u32` — nothing bigger can land on
the volume anyway).

```
0x00  8   magic "UNAOSUP1"
0x08  4   entry count            (1..=64)
0x0C  4   header_len             (16..=65536; data region starts here)
0x10  …   entry table, packed:
            u16 path_len (1..=128) | path (UTF-8, '/'-separated, every component 8.3)
            u32 size | 32-byte sha256 of the file's content
      …   data region: each entry's bytes concatenated in entry order, unpadded
```

Invariant checked at parse: `header_len + Σ size == file size`, **exactly** — a truncated or padded
delivery is refused even after the whole-file sha matched (belt and braces: `UPDATE.SHA` guards the
transport, the equation guards the container's own arithmetic).

## 5. Update-flow state machine

Runs once per boot from `tegra_early_stop`, after ORIN-INSTALL-2's site (same preconditions: the
JB2b pump has enumerated the boot volume as a block device; still at EL2 with the JM4 timer live).
Every stage is witnessed on serial as `[orinselfup] S<n>`; every failure is a witnessed refusal,
never a panic.

```
S0 SCAN    mount() the boot volume; honour fs.write_veto(); look for UPDATE.PAK + UPDATE.SHA.
           Absent => one witness line, normal boot (the common case).
S1 VERIFY  stream sha256 over the whole UPDATE.PAK (32 KiB chunks); compare UPDATE.SHA.
S2 PARSE   decode UPK1; refuse BEFORE any write on: bad magic, bounds, non-8.3 components,
           duplicate/reserved paths, the exact-consumption equation, missing pair (§6).
S3 WRITE   per file: resolve/create parent dirs; sweep a stale temp; write bytes to UPD<i>.TMP
           beside the live file (hashing in flight); RE-READ the staged file off the volume and
           hash again. Any mismatch/short read/short write => abort, delete all temps, live set
           untouched.
S4 FLIP    per file: delete live entry, rename temp onto the live name. Order: every non-pair
           file, then EFI/BOOT/BOOTAA64.EFI, then KERNEL.ELF LAST. Pair window witnessed
           OPEN/CLOSED.
S5 CLEAN   delete UPDATE.PAK + UPDATE.SHA (a consumed payload cannot re-apply next boot).
S6 REBOOT  reboot_hook() — the warm-reboot seam (§8).
```

The writes go through the standing `fs::fat` write path (`create_in_dir` / `write_grow` /
`rename_entry` / `delete_located`) — the path the July 2026 bench replays proved on this board,
power-cycle survival included. No new writer, no new sector-level call site: for the x86 FAT-mutator
roster this is the existing file-verb class (mounted boot volume, boot core, pre-JM6 single-threaded
window), not a new when/where for `write_sectors`.

## 6. Matched-pair enforcement

Twice, deliberately redundant:

1. **Parse gate (S2)**: a payload without BOTH `EFI/BOOT/BOOTAA64.EFI` and `KERNEL.ELF` is refused
   before the first write. There is no file-level update verb to misuse — the only unit the code can
   apply is the whole container, and `selfup-pak` refuses to build a partial one.
2. **Flip order (S4)**: bytes for every file are already on the volume before the first flip; the
   pair flips last and adjacent, loader before kernel — so at no instant does a fresh kernel sit
   beside an old loader (the rule's named forbidden state).

## 7. Failure modes

* **Power loss during S3 (the long phase — multi-MiB writes)**: the live boot set is untouched;
  `UPD*.TMP` litter is swept by the next armed run before re-staging. Safe by construction.
* **Power loss during S4 (the short phase — two directory-entry RMWs per file)**: the residual
  window. A partially flipped ESP is possible; recovery is boot-media rebuild from the host. The
  window is minimized (data already durable; only dir-entry renames remain) and ordered per §6, but
  **closing it needs loader-side A/B** (e.g. the loader trying a `.NEW` kernel first, or a GPT
  attribute flip) — recorded as future work, not silently accepted. Note the USB recovery stick
  workflow survives regardless: a rebuilt stick boots the board no matter what state the flip died
  in.
* **sha mismatch** — S1 (whole payload), S3 streaming (per file), S3 read-back (what the medium
  actually holds): refuse/abort with the live set intact; the payload is left in place for
  inspection (only a *successful* update consumes it).
* **Short read / truncation**: S2's exact-consumption equation refuses a truncated container; a
  cluster chain ending early during any streaming pass is a witnessed short read and aborts.
* **Integrity vs authenticity**: sha256 here guards against transport corruption, NOT a hostile
  payload — anyone who can write the staging files can write the boot files directly on this bench.
  Payload signing belongs with the SECURITY.md hardening ledger once a transport crosses a trust
  boundary (LAN); flagged there when that lands.

## 8. The reboot seam

`selfup_tegra::reboot_hook()` is the single agreed call site for the warm-reboot verb, which the
exec-reboot arc is building in parallel. Until it lands the hook is a witnessed no-op (`S6 reboot —
warm-reboot verb NOT WIRED yet`): the boot continues on the in-RAM kernel and the updated ESP takes
effect at the next power cycle. Wiring = replace the hook body with one call.

## 9. Knob, gating, witnesses

* `UNAOS_SELFUP=1` ⇒ feature `selfup` — STANDALONE (does not imply `tegra`), mirroring
  `smpmark`/`orindesk`/`jd1dc`; every site is tegra-gated, so arm it WITH tegra:
  `UNAOS_TEGRA=1 UNAOS_SELFUP=1 ./arroyo check` or `UNAOS_SELFUP=1 ./arroyo esp-jetson`.
* Default OFF ⇒ the module and its appended call-site statement vanish; the jetson image is
  byte-identical to baseline. The ARMED polarity is type-checked by the `arm-tegra-selfup` leg of
  KERNEL_CFG_MATRIX.
* Witness family: `[orinselfup]` — a 12-byte prefix, over the 8-byte LLVM immediate-encoding bound,
  so `strings` on the built artifact proves the lines are real data, not folded immediates.

## 10. Bench runbook (arc-boundary verification, not this arc's gate)

1. `./arroyo esp-jetson` (new tree) → `./arroyo selfup-pak` → copy `UPDATE.PAK` + `UPDATE.SHA` into
   the CURRENT boot stick's root (card loop).
2. Boot a `UNAOS_SELFUP=1 ./arroyo esp-jetson` image; capture serial.
3. Expect the S0→S5 ladder, `UPDATE APPLIED`, the S6 not-wired line (until exec-reboot lands).
4. Power-cycle; the board must boot the NEW pair; re-run and expect the `no staged payload` S0 line
   (payload consumed).
5. Negative legs: flip one byte in `UPDATE.PAK` (S1 refusal), delete one pak entry at build by
   pruning the tree (builder refusal), truncate the pak (S2 equation refusal).
