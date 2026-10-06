# FWPIN — the b43 microcode set is hash-pinned, searched on the boot card only, and a malformed image is refused (rmbp-ledger B455)

**Finding (SEC-2026-10-06 F5, MED; HIGH once `wifi2` uploads).** `wifi/firmware.rs` staged the three-file b43 set
(`ucode29_mimo.fw`, `ht0initvals29.fw`, `ht0bsinitvals29.fw`) by NAME alone: FNV-1a was printed, never compared; a
`stream=violates-layout` image was still STAGED; and `block::alternate_program_source` handed the search a SECOND
volume — after USBSTOR (B384) the USB stick itself — so a stick carrying `ucode29_mimo.fw` fed microcode to a
bus-mastering PCIe device. A second gap read off the code while tracing the search: arroyo's WIFI-FW block puts
the set on the card's **UnaFS** root at `/FIRMWARE/`, but the loader mounted only the program source's **FAT**
(the ESP, p1) — so a ROOTDISK2 card built with `UNAOS_WIFI_FW_PATH` could never stage the set it carries.

**Peter's rule (the ledger row):** the firmware is bench-only, from the bunker, through `UNAOS_WIFI_FW_PATH` onto the
card's UnaFS `/FIRMWARE/`. R95: a removable volume is data, never a program source for microcode.

**Seam.** `wifi/` is a Kernel driver (CHARTER: Kernel — driver); no new file under the charter dirs. The pin is a
fact about the user's files, held in `unaos/firmware/b43.pins` (role + SHA-256, no bytes) and compiled in with
`include_str!`; SHA-256 is the shared `crypto_core::sha2` the kernel already links (the `hash` module's own source).

**Milestones.**
- **M1 — pin + layout.** Every candidate is SHA-256'd and compared with its role's pin BEFORE it enters `STAGED`:
  no pin → `REJECTED … reason=unpinned sha256=<hex>`; a different digest → `reason=pin-mismatch sha256=<hex>
  pin=<hex>`; `stream=violates-layout` (or an unrecognized header) → `reason=violates-layout`. Only a match stages
  (`STAGED … sha256=<hex> pin=match`). `b43.pins` ships EMPTY: no hash is known on this machine and none is invented.
- **M2 — the boot card only.** The alternate (USB) pass is gone. Pass 2 is the boot card's own UnaFS root, read
  through the mount table (`/`, `/B43/`, `/FIRMWARE/`), and only when `/` is the native volume AND
  `unafs::mount_bound_handle()` is the program-source handle; otherwise the pass says why and searches nothing.
  WIFI-REACH's `Pending` is no longer returned (no second volume is awaited).
- **M3 — the witness and the build line.** `tests wifi` (WIFI5) gains `fw=pinned sha=match` or
  `fw=refused reason=<unpinned|pin-mismatch|violates-layout>`, and `NOT-READY reason=` names the refusal. arroyo's
  WIFI-FW block prints `⚡ WIFI-FW: <file> sha256=<hex> bytes=<n>` per file and the paste-ready pin rows, and says
  whether `b43.pins` already matches.
- **M4 — docs.** SECURITY.md §Process & supply chain row; bcm4331.md §F.

**Witness (what the next flight reads, in order).**
1. Build line: `⚡ WIFI-FW: ucode29_mimo.fw sha256=<64 hex> bytes=39760` (+ the two initvals), then
   `⚡ WIFI-FW: b43.pins … -> UNPINNED` on the first build.
2. First staged boot: `:: wifi: ucode REJECTED /FIRMWARE/ucode29_mimo.fw size=39760 on boot-root … — reason=unpinned
   sha256=<hex> ::` ×3, `firmware set INCOMPLETE 0/3`, and `:: WIFI5: … fw=refused reason=unpinned -> NOT-READY
   reason=unpinned ::`. The seat pastes the three build-line hashes (they must equal the kernel's) into `b43.pins`.
3. Second boot: `STAGED … sha256=<hex> pin=match` ×3, `COMPLETE 3/3`, `:: WIFI5: … fw=pinned sha=match -> READY ::`.

**Owed.** The `S_WAIT_ALT` state in `wifi/mod.rs` is now unreachable (Pending is never returned) and
`block::alternate_program_source` has no caller; both are left for the integrator to retire rather than churn a
shared file in this arc. The hashes themselves: Peter's first `UNAOS_WIFI_FW_PATH` build prints them.
