# ORIN-INSTALL-2 — landing report

**Arc:** INSTALL-2 — the self-clone: the installer copies the RUNNING system's real boot payload. Rung 3b of
the installer line, on top of INSTALL-1 (`review/unaos-install1-LANDING.md`). Same `UNAOS_INSTALL_TARGET_SD=1`
gate.

**Track:** hw-jetson. **Base:** hw-jetson, level with `main` (0 ahead / 0 behind at session start; clean).

## What landed

INSTALL-1's honest-but-synthetic `UNAOS.IMG` marker is replaced by the **real thing**: the installer mounts the
USB boot stick's own ESP and mirrors its boot tree onto the freshly-formatted microSD ESP, every copied file
sha-extent-verified. Two structural changes make it possible: the destructive install is **repositioned** to
after the USB stick enumerates, and the installer **engine gains a multi-file / multi-cluster / subdirectory
writer** (the flagged single-FAT-sector bound lifted). All additive; every battery unregressed; non-install
builds byte-identical.

### Files

- `unaos/crates/kernel/src/install/fat32.rs` — **additive** `TreeWriter` (free-cluster cursor, multi-FAT-sector
  `set_fat_run`, in-memory directory clusters, `write_file`), `put_dir_entry`, and the `ATTR_*` /
  `DIR_SLOTS_PER_CLUSTER` consts. `write_payload_file` (the x86 witness's path) is **untouched**.
- `unaos/crates/kernel/src/arch/aarch64/sdmmc_tegra.rs` — `PENDING_INSTALL` stash; `sdmmc_install_from_usb`
  (the deferred entry, re-exported); rewritten `install_to_sd`/`install_flow` (mount USB → `copy_dir` tree
  clone → per-file sha manifest); `copy_dir` (recursive tree mirror) + `sha_hex`; `Card` gains
  `#[cfg_attr(install_target, derive(Clone, Copy))]`; census now stashes instead of installing; virt stub line
  → INSTALL-2. All new code `install_target`-gated.
- `unaos/crates/kernel/src/main.rs` — the deferred install call site: **removed** from inside `sdmmc_census`
  (~1396, now read-only), **added** after the `if xusb_alive { … }` JB2b block (~line 1522), gated
  `#[cfg(all(feature = "install_target", feature = "tegra"))]`.
- `docs/dev/OS/01_BOOT_HAL/arch_arm64.md` — §ORIN-INSTALL-2 subsection.
- `unaos/docs/dev/OS/10_INSTALL/installer_engine.md` — §INSTALL-2 section + header line.
- `unaos/scripts/orin-sdmmc1-bench.md` — install leg refreshed (deferred site, new wire chain, verify-FAIL /
  SKIP shapes, host-reader boot-tree confirmation).
- `review/unaos-install2-LANDING.md` (this file).

## M1 — the position adjudication

INSTALL-1's blocker: `install_to_sd` ran at the pre-JB2b EL2 census site where `drivers::block::info()` is
`None` (the USB boot stick is not yet enumerated), so the running boot payload was unreadable. INSTALL-2 splits
the act:

- the read-only `sdmmc_census` still runs pre-JB2b (`main.rs` ~1396) and now **stashes** `(base, Card, sector0)`
  into `PENDING_INSTALL`;
- the destructive install is **deferred** to `sdmmc_install_from_usb`, called **immediately after the JB2b pump
  window** (`main.rs` just after the `if xusb_alive { … }` block, ~line 1522).

That is the earliest position where all three constraints hold:
- **(a) the stick is readable** — the JB2b pump's `service_storage` publishes `drivers::block::BLOCK_DEVICE` in
  its settle window, so `info()` is `Some`. The tegra build is **not** `baremetal` (`baremetal = ["pi"]`), so
  `drivers::block` routes to the xHCI USB-MSC path — never the Pi `emmc2` SD backend — so there is **no backend
  conflict** with the directly-MMIO-driven `SdInstallTarget`.
- **(b) the SDMMC MMIO is still usable** — the census-mapped GiB-0 Device-nGnRE window persists, and the core is
  **still at EL2** here (the JM6 drop is further down), so the SD path's bounded `hlt()` waits still have the
  JM4 timer as their wake source.
- **(c) nothing later is perturbed** — the JD2 console shell reads `drivers::block` (the USB stick), not the SD;
  the SMP wake touches neither. The microSD is repartitioned in isolation.

No EL-drop / takeover unsafety was found at this position (it is pre-drop, at EL2, MMIO mapped) — so the
post-JB2b site was taken, not refused. Honest SKIP paths (no card stashed, or stick not enumerated) print and
do nothing destructive.

## M2 — the payload enumeration + engine bound extension

`install_flow` mounts the stick (`fs::fat::mount()`), then `copy_dir` walks its root recursively and mirrors the
tree: the esp-jetson layout is `/EFI/BOOT/BOOTAA64.EFI` + `/kernel.elf`, so the copy recreates the `EFI/` and
`EFI/BOOT/` subdirectories (well-formed `.`/`..`, `..`→0 for a root child per the FAT convention) and both
files. Nothing is hardcoded beyond skipping `.`/`..` — whatever the stick carries is enumerated and cloned;
each file is read whole (32 MiB per-file cap vs the 48 MiB heap), clusters allocated + written, sha recorded.

**Engine bound extension (the flagged single-FAT-sector bound, lifted).** INSTALL-1's `write_payload_file`
capped a chain at FAT sector 0 (≤125 clusters ≈ 64 KiB) — far too small for a real `kernel.elf`. Added
**additively** as `TreeWriter` (the x86 witness's `write_payload_file` untouched):
- a running free-cluster cursor (many files/dirs allocate distinct chains);
- `set_fat_run` — links a chain across **every FAT sector it touches, in both FAT copies** (multi-FAT-sector
  chains: a multi-MB image links correctly), RMW once per touched sector per copy;
- directory clusters built **wholly in memory** then written once (a stale data cluster on a non-blank card
  never leaks bytes into a directory; each dir assumed ≤ one cluster — the boot tree is; overflow = honest
  `NoSpace`, not truncation).

Verify discipline unchanged: every copied file is re-read off the card and SHA-checked through the engine's own
`verify_extents`, and the flow prints a **per-file `sha256=… VERIFIED` manifest** — the installer's
content-verify IS the bench's content-verify, now native. The `UNAOS.IMG` marker is gone.

## M3 — the QEMU story (honest, unregressed)

No Tegra234 xHCI/SD in QEMU: virt `install_target` builds print one honest compiled-present metal-only line and
do zero MMIO; the deferred call site is tegra-gated (absent on virt). The x86 `UNAOS_INSTALLDEMO` engine witness
is unperturbed — `write_payload_file` is unchanged, and the `TreeWriter` additions are unused by it — so it
still runs GPT→FAT32→copy→verify→negative-test→PASS.

## Gate results (verbatim)

- **`./arroyo check` — default (both arches):** 0 errors.
- **`UNAOS_INSTALL_TARGET_SD=1 ./arroyo check` — virt (both arches):** 0 errors.
- **`UNAOS_INSTALL_TARGET_SD=1 UNAOS_TEGRA=1 ./arroyo check` — tegra (both arches):** 0 errors, no new warnings
  in any touched file.
- **`UNAOS_INSTALLDEMO=1 ./arroyo check`:** 0 errors (the `TreeWriter` additions compile in the engine build).
- **`UNAOS_INSTALLDEMO=1 ./arroyo test` (x86 engine):** engine end-to-end PASS —
  `:: INSTALL: gpt+fat32+copy verify => PASS ::`, 0 real FAIL.
- **`./arroyo test` (x86):** 0 FAIL.
- **`./arroyo test-arm` (plain GICv2):** 0 FAIL.
- **`UNAOS_GICV3=1 ./arroyo test-arm` (CAPSTONE):** `CAPSTONE COMPLETE — all 6 sync primitives verified in one
  boot`; Semaphore/Mutex/Channel/Condvar/RwLock/join all PASS; 0 FAIL.
- **`UNAOS_INSTALL_TARGET_SD=1 UNAOS_GICV3=1 ./arroyo test-arm`:** the three SDMMC/INSTALL compiled-present
  metal-only witness lines print (incl. `ORIN-INSTALL-2 third gate … compiled-present but metal-only`), CAPSTONE
  6/6, 0 FAIL.
- **`./arroyo kernel8-test 35` (Pi bare-metal gate):** full K-suite PASS (K1–K9, FATDIRS, FATMOVE, IMG-SIG,
  K8a/b/c CoW, …), 0 FAIL.
- **String-identity (esp-jetson builds, established method):** the `sdmmc_arm` jetson binary contains **0**
  `ORIN-INSTALL` strings and **rebuilds byte-for-byte identical** (install code fully gated out); the
  `install_target` jetson binary contains the install strings (`ORIN-INSTALL-2` / `self-clone` / `ABOUT TO
  DESTROY`).

## Metal leg (owed — next Orin sitting, attended)

The full self-clone's **first execution is the attended Orin sitting**: boot
`UNAOS_INSTALL_TARGET_SD=1 UNAOS_TEGRA=1 ./arroyo esp-jetson` on a card the operator is willing to erase, with a
USB keyboard/stick that lets JB2b enumerate storage — confirm the ABOUT-TO-DESTROY line, watch the per-file
`sha256=… VERIFIED` manifest, reach `ORIN-INSTALL-2 SD install … => PASS`, then re-seat the card and confirm a
host reader sees the cloned boot tree (`/EFI/BOOT/BOOTAA64.EFI` + `/kernel.elf`) with matching host-side
sha256s. Runbook: `unaos/scripts/orin-sdmmc1-bench.md`, install leg.

## Flagged

- **Bootability:** the card carries a faithful boot tree, but making the Orin actually boot from it (ESP type
  GUID / boot attributes / firmware boot-order) is the next rung's metal question — out of this arc's scope.
- **Throughput:** single-block CMD24/CMD17 only; a multi-MB `kernel.elf` is many single-block writes plus the
  metadata-zero pass. Multi-block CMD25/CMD18 remains the named perf follow-up (correct, not fast).
- **Directory size:** the tree writer assumes each directory fits one cluster (the boot ESP does). A payload
  with >16 entries in one directory would hit an honest `NoSpace` — multi-cluster directories are a follow-up if
  a future payload needs them.
