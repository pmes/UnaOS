# ORIN-SDMMC-2 — landing report

**Arc:** ORIN-SDMMC-2 — the SD **write path** behind the paranoia ladder. The installer line's second rung
(`~/.claude/plans/unaos/future/unaos-installer.md`), extending the read-only ORIN-SDMMC-1 recon. Law: THE
SEATED CARD IS SACRED.

**Track:** hw-jetson. **Base:** hw-jetson, level with `main` (0 ahead / 0 behind at session start; clean).

## What landed

The write path in `arch/aarch64/sdmmc_tegra.rs`, **double-gated** behind a new `sdmmc_arm` cargo feature
(requires `sdmmc`) wired to `UNAOS_SDMMC_ARM=1`, plus its doc refresh. A card write happens only when BOTH the
`sdmmc` feature AND the separate `sdmmc_arm` arm are present. Committed in three steps:

- **M1 (`7f442bc`) — arm gating + ladder steps 1-3, ZERO card writes.** The `sdmmc_arm` feature + knob, the
  virt armed-witness line, an arm-gated generalised single-block reader (`read_block_at`), a 512-byte hex
  dumper (`dump_hex`), and the read-only front of the ladder: re-run the rung-1 read census (step 1), pick the
  scratch region under the GPT-refusal rule (step 2), read + stash it (step 3).
- **M2 (`c482f7c`) — the CMD24 write + ladder steps 4-7.** The single-block `WRITE_SINGLE_BLOCK` primitive
  (`write_block_at`, the driver's only card-write), the emergency-restore helper (`restore_or_dump`), the
  stamped-pattern builder (`make_pattern`), and steps 4 (write) / 5 (verify) / 6 (restore) / 7 (restore
  verify) with the `=> PASS` line.
- **M3 (this commit) — docs.** arch_arm64.md §ORIN-SDMMC extension (the SDMMC-2 subsection), the combined
  recon+armed-ladder runbook `scripts/orin-sdmmc1-bench.md`, the Cargo/arroyo knob comments, and this report.

### Files

- `unaos/crates/kernel/src/arch/aarch64/sdmmc_tegra.rs` — the write path (all `sdmmc_arm`-gated, appended
  below a clearly-marked ORIN-SDMMC-2 banner; the rung-1 read functions are untouched).
- `unaos/crates/kernel/Cargo.toml` — `sdmmc_arm = ["sdmmc"]`.
- `unaos/arroyo` — `[ -n "${UNAOS_SDMMC_ARM:-}" ] && _feats="${_feats}sdmmc_arm,sdmmc,"`.
- `docs/dev/OS/01_BOOT_HAL/arch_arm64.md` — §ORIN-SDMMC-2 subsection.
- `unaos/scripts/orin-sdmmc1-bench.md` — refreshed into the combined unarmed-recon + armed-ladder runbook.
- `review/unaos-orin-sdmmc2-LANDING.md` (this file).

## The scratch-region rule (and the GPT refusal)

**Scratch region = the card's last block (LBA `capacity-1`), single-block, ONLY IF sector 0 shows no GPT.**

- **GPT present ⇒ REFUSE all scratch writes this arc.** A GPT **backup** header lives in the card's last LBA
  — exactly where a last-block scratch region sits. So on any card whose sector 0 classifies as a
  GPT-protective MBR, the ladder prints `write ladder REFUSED (GPT present — the seated card is sacred; no
  write)` and stops before any write. (Extending the write to a mid-card region blindly is more dangerous, not
  less; a GPT-aware target-region arc is future installer work.)
- **No GPT ⇒ last block is the scratch region.** Without GPT there is no end-of-device structure (GPT backup)
  to endanger; MBR/FAT partitions essentially never extend to the very last LBA of the device. And the write
  is **stashed-then-restored** regardless, so the worst case — a power loss between the write (step 4) and the
  verified restore (step 7) — can only leave a stamped pattern in an end-of-device block that held no
  partition or backup structure, and the emergency-restore + hex-dump path preserves the original bytes on
  serial. This is why the last block is the provably-safe choice when (and only when) there is no GPT.

Single scratch block (`SCRATCH_BLOCKS = 1`); the witness uses single-block CMD24 only (multi-block CMD25/CMD18
is not used this arc).

## Unarmed-identity evidence (byte-identical to merged rung 1)

Every line of the write path is `#[cfg(feature = "sdmmc_arm")]`-gated, and `sdmmc_arm` is pulled only by
`UNAOS_SDMMC_ARM=1`. Concretely:

- **No SDMMC-2 code or strings in the unarmed kernel.** Built `--features tegra,tegrasmp,sdmmc,ehcihid,smolnet`
  (the tegra recon config, no arm): `strings … | grep -cE "ORIN-SDMMC-2|ladder step|write ladder|write/verify"`
  = **0**. The rung-1 strings are intact (`grep -c "ORIN-SDMMC-1"` = 4).
- **Serial identity.** Knob-off `UNAOS_GICV3=1 ./arroyo test-arm 40`: 0 `SDMMC` strings, 0 FAIL. Armed
  `UNAOS_SDMMC=1 UNAOS_SDMMC_ARM=1 UNAOS_GICV3=1 ./arroyo test-arm 40`: exactly the two virt-witness lines,
  CAPSTONE 6/6 intact.
- **On the whole-file binary hash:** it does differ between the pre-arm and no-arm-post-arc builds
  (`edffe19…c75210` → `ba1889df…`), but **only because adding `sdmmc_arm` to Cargo.toml changes the crate's
  feature-fingerprint**, which the compiler embeds in symbol-mangling hashes; the *active* code is unchanged,
  as the zero-SDMMC-2-strings result proves. A binary-hash match is unachievable once the feature exists in
  the manifest, so the string/serial evidence is the correct identity test.

## Gate results (verbatim)

- `./arroyo check` (default, both arches): `✅ x86_64 OK` / `✅ aarch64 OK`.
- `UNAOS_SDMMC=1 ./arroyo check` (virt): `✅ x86_64 OK` / `✅ aarch64 OK`.
- `UNAOS_SDMMC=1 UNAOS_TEGRA=1 ./arroyo check`: `✅ x86_64 OK` / `✅ aarch64 OK`.
- `UNAOS_SDMMC=1 UNAOS_SDMMC_ARM=1 ./arroyo check` (armed virt): `✅ x86_64 OK` / `✅ aarch64 OK`.
- `UNAOS_SDMMC=1 UNAOS_SDMMC_ARM=1 UNAOS_TEGRA=1 ./arroyo check` (armed tegra): `✅ x86_64 OK` /
  `✅ aarch64 OK`.
- knob-off `UNAOS_GICV3=1 ./arroyo test-arm 40`: `CAPSTONE COMPLETE — all 6 sync primitives verified in one
  boot` (Semaphore/Mutex/Channel/Condvar/RwLock/join all PASS); 0 FAIL; 0 `SDMMC` strings.
- `./arroyo test-arm 22`: `✅ aarch64 test complete`; 0 uppercase FAIL.
- `./arroyo kernel8-test`: `✅ Flashable image` + `CAPSTONE COMPLETE`; 0 FAIL.
- `./arroyo test 22` (x86): `✅ Test run complete`; 0 FAIL.
- armed virt witness `UNAOS_SDMMC=1 UNAOS_SDMMC_ARM=1 UNAOS_GICV3=1 ./arroyo test-arm 40`: prints
  `:: SDMMC: ORIN-SDMMC-1 … recon is metal-only …` and `:: SDMMC: ORIN-SDMMC-2 write ladder ARMED
  (UNAOS_SDMMC_ARM=1) but metal-only … zero card writes here ::`; CAPSTONE 6/6 intact.

## Flagged / metal-pending

- **QEMU cannot model the Tegra234 SDMMC** — the ladder's metal path (steps 1-7 against a real card) is
  code-complete-prior-to-metal. Correctness off-metal rests on `arroyo check`, the QEMU non-regression (the
  tegra write path compiled out on virt), the emmc2 register model, and SD-spec/SDHCI adherence. The metal leg
  (an armed ladder run against a seated **non-GPT** card, reaching `PASS`, and a GPT card reaching the honest
  `REFUSED`) is the next Orin sitting; runbook `scripts/orin-sdmmc1-bench.md`.
- **The write is confined to the last block of a non-GPT card, stashed + restored + verified.** No general
  installer write yet — that is INSTALL-1 (rung 3): GPT partition + FAT format + payload write, which will need
  a GPT-aware target-region model rather than the last-block scratch rule.
- **Lane:** stayed in-lane — `sdmmc_tegra.rs` + the `sdmmc_arm` knob wiring (Cargo/arroyo) + the named docs.
  Did NOT touch the Pi `emmc2` driver, pcie/net files, sched, xhci, or any file outside the brief.

## Commits (on `hw-jetson`; not merged, not pushed)

- `7f442bc` — `sdmmc: ORIN-SDMMC-2 M1 — write ARM gating + paranoia ladder steps 1-3 (no card writes)`
- `c482f7c` — `sdmmc: ORIN-SDMMC-2 M2 — the CMD24 write + paranoia-ladder steps 4-7`
- (M3) — `sdmmc: ORIN-SDMMC-2 M3 — docs (arch_arm64 §SDMMC-2 + combined runbook + landing)`
