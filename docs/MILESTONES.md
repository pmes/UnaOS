# UnaOS milestones

A running, quick-to-digest log of what landed each integration round — one
entry per arc, newest first. Each entry: **what it does**, **how it was tested**
(QEMU + metal), and the commit. Deep detail lives in the per-subsystem docs
under [`dev/OS/`](dev/OS); the ledger of hardening state is in
[`SECURITY.md`](SECURITY.md); direction is [`ROADMAP.md`](ROADMAP.md).

Legend: **✅ metal-confirmed** · **🔬 QEMU-green, metal pending** · dates ISO.

---

## hw-jetson track — 2026-07-10 (JD5 — the write path: the panel becomes a real workstation shell; same-day attended bench — PASS)

### JD5 — `touch` / `write` / `append` / `rm` / `sync` on the panel shell ✅ METAL-CONFIRMED (2026-07-10) `hw-jetson`
- **What (M1, arch-neutral, `3a143f5`):** `touch <path>` creates a 0-length root file (idempotent);
  `write <path> <text>` is create-or-TRUNCATE storing the exact bytes (truncate = `delete_located` +
  `create_in_root` + `write_grow` — the only create-or-truncate through the PUBLIC API; no in-place
  shrink primitive). The raw `write <lba> <byte>` stays byte-identical for its 2-numeric-arg shape;
  any other shape is the file write. Rides `fat.rs`'s F3-locked PUBLIC mutation API directly (the SVC
  path is EL0/ASID-keyed + out-of-lane; the shell runs at EL1 as **ASID 0** → an ASID-0 create is
  PUBLIC by U6's rule, so shell files are plain public FAT files). **Root-directory only** —
  `create_in_root`/`find_located` are root-only and `fat.rs` is call-never-edit, so a subdir target
  is an honest `-ENOTSUP`.
- **What (M2, arch-neutral, `dfaf180`):** `append <path> <text>` = open-seek-end-write via
  `write_grow` (create-if-absent, like `>>`); `rm <path>` (alias `del`) = `delete_located` (mark
  `0xE5` first, then free the chain — crash-safe order), directory → `-EISDIR`, absent → `-ENOENT`.
- **What (M3, `2531209`):** safety rails made explicit — writes are BOUNDED (`block::write_block`
  rides the SAME JD3 wall-clock BOT pump as reads, so a stalled write times out to `-EIO`, never
  WFI-parks the timerless EL1 core — verified in the driver), CONSISTENT (each `fat.rs` step atomic
  under F3's `FAT_MUTATION`/`DIR_MUTATION`; a mid-sequence failure leaves lost clusters / the old
  smaller size, never a torn volume), and WRITE-THROUGH (`sync` is an honest no-op — no cache).
- **Tested (QEMU):** `check` + `UNAOS_TEGRA=1 check` green both arches; `UNAOS_HUBSTORAGE=1 test 25`
  MISSION (shared `shell.rs` guard); `test-arm 22` MISSION; `UNAOS_GICV3=1 test-arm 40` CAPSTONE 6/6;
  `esp-jetson` links, **108 `tegra:` strings** (unchanged — shell-write strings carry no `tegra:`
  token; validate by count, not size). The write PRIMITIVES already run headless via the
  `el0-u10create`/`u10delete`/`u11close` fixtures (identical `create_in_root`/`write_grow`/
  `delete_located`); the SHELL arms are thin glue, dispatched only on a keystroke, so a headless
  shell-write demo is not in-lane — verdict attended-pending like JD2/JD3/JD4.
- **Metal — ✅ PASS (2026-07-10 attended bench, serial `jetson-serial-2026-07-10-165211.log`):** the
  whole battery on the FAT16 `UNAOSRW` card (29 MiB, Alcor reader behind the hub, slot 5; clean enum
  both boots) — boot 1 `write hello.txt` → `cat` → `append` → `cat`, **power-cycle**, boot 2
  `cat hello.txt` **survives** (write-through durability on silicon), `rm` → `-ENOENT`,
  `write docs/x.txt`/`docs/y.txt` → `-ENOTSUP` (file confirmed never created; `ls docs` still works),
  all typed lowercase (case-insensitive 8.3). Zero `BOT pump TIMEOUT`/errors/panics both boots. ⚠ the
  rMBP `UNAOS` card was repurposed as the Orin boot stick — rMBP must re-flash its boot media.
- **Detail:** [`arch_arm64.md` §JD5](dev/OS/01_BOOT_HAL/arch_arm64.md). **Commits:** `3a143f5` M1 ·
  `dfaf180` M2 · `2531209` M3 (`hw-jetson`).

## hw-pi4 track — 2026-07-10 (K1 M1 — the on-disk owner/grants format; persistence stops for a seat decision)

### K1 M1 — `UNAFS.ATR`: persist the U6 ACL across reboot — FORMAT + round-trip 🔬 `hw-pi4`
- **What:** U6's owner/grants ACL is in-RAM and boot-scoped (the owner is an `(asid, gen)`
  incarnation), so a power-cycle reverts every private file to PUBLIC. K1 persists owner + grants
  as on-disk attributes inside a reserved hidden|system FAT file `UNAFS.ATR`. **M1 lands the on-disk
  FORMAT** (a versioned magic header + 16 bounded 256-byte rows — mirroring the 16-row in-RAM table
  — each with a 32-byte kind-tagged owner `PrincipalRecord` + 4 grants, per-header and per-row CRC32,
  a volume binding; single-row `write_at` = one-sector RMW), **its (de)serializers, reserved-file
  helpers built entirely on `fat.rs`'s existing public API (zero `fat.rs` edit), and a round-trip
  self-test** — codec (populated-row byte/field round-trip + CRC-corrupt/bad-magic/wrong-binding
  fail-closed) and on-disk (create the empty image, write one SYNTHETIC row via `write_at` UNDER the
  F3 `NAMESPACE` lock, read back byte-equal, clear it → a valid all-public image).
- **STOP-GATE:** the persistent-principal model is genuinely undefined — a persisted owner cannot be
  a boot-scoped `(asid, gen)`, and UnaOS has no persistent principal (EL0 blobs loaded by name, no
  code-signing / manifest / RTC / uid). Defining it is a TCB-level policy decision above the pi4
  aarch64 lane, so **M1 STOPS and PROPOSES to the seat** a launcher-assigned, kernel-stamped
  principal (default `PROGRAM_NAME`), and wires NO enforcement (enforcement-inert: the 23-PASS
  battery is byte-identical; K1 emits its own `:: K1-atr: ::` line, not a `-> PASS`). Persist +
  mount-rebuild + enforcement are M2/M3/M4, seat-gated.
- **Tested (QEMU):** `check` green both arches (zero x86 change); `kernel8` compiles baremetal;
  `kernel8-test 40` → **23 PASS byte-identical** (sorted diff vs base `f2ad34c`) + CAPSTONE 6/6 +
  F2-witness (240000/240000 locked) + F3-witness (240000/240000 locked) + `:: K1-atr: codec PASS …
  on-disk helpers disk PASS … ENFORCEMENT-INERT ::`, zero R1/CMD13; `test-arm 30` MISSION SUCCESS.
- **Metal:** the disk round-trip creates a real `UNAFS.ATR` on the card (valid, empty/all-public) —
  metal-verify at the next attended bench (a fresh-card re-prep should delete `UNAFS.ATR` too).
- **Detail:** [`SECURITY.md` §K1 M1](SECURITY.md) (the full design section: principal proposal,
  format, fail-closed asymmetry, lock discipline, seat question). **Commit:** the single K1 M1
  commit on `hw-pi4` (see `git log`).

## hw-rmbp track — 2026-07-10 (STOR-1 S1–S3 — interrupt-driven x86 storage behind the `irqstorage` knob; ✅ core mechanism METAL-CONFIRMED)

### STOR-1 S1–S3 — the storage service task + live reads + live in-place write-through ✅ core mechanism METAL-CONFIRMED (2026-07-10) `hw-rmbp`
- **What:** x86 storage syscalls run IF-masked (SFMASK), and the only kernel→sector path is the xHCI
  BOT pump, which `hlt()`s awaiting a transfer event — a `hlt` at IF=0 never wakes, so an in-handler
  disk op hangs the core. The whole staged-buffer family (U6bx staged read / U9x flush queue / U10x
  op-queue) exists to defer disk work to an IF=1 context. STOR-1 lays the IF-safe replacement spine
  (design `unaos/docs/dev/OS/07_USB_STORAGE/x86_interrupt_storage.md`): a scheduled kernel **storage service
  task** (IF=1) owns the BOT pump; a syscall builds a `BlockRequest` on its kernel stack, wakes the
  service task, and **blocks on a per-request semaphore** whose `wait` restores the caller's IF
  snapshot across the switch — so the handler sleeps with IF=1 semantics though it entered IF-masked
  (the crux). All wakeups happen at IF=1, so the wake-only MSI-X handler stays lock-free
  (`sched.rs:28` preserved). **S1** (`7b2f05f`): the service task + submit/block/complete + a
  `bx-blockreq` self-test (LBA0 polled vs service-task read → PASS); a bounded pump timeout →
  `BlockError::Io`, never a hang. **S2** (`b73ba08`): `sys_read` serves a RO staged descriptor from
  the LIVE volume (name-based `find_located`+`read_at`), bounced through a kernel-stack buffer (the
  ring-3 window is the submitter's private CR3 = PML4[2], unreachable by the service task under kernel
  CR3; the kernel stack is in the shared kernel half). **S3** (`fd5de85`): `sys_write_file` writes
  THROUGH in place for a non-growable staged descriptor (SCRATCH.BIN) — synchronous `write_at`, no
  wstage/dirty/FLUSH-queue; the matching read broadens to `FILE_OPNAME==0`; a new witness proves the
  write is on disk pre-drain with an empty flush queue → **close-discards-dirty residual retired**.
- **Knob:** everything behind the `irqstorage` cargo feature (`UNAOS_IRQSTORAGE=1`; mapped in
  `builder` + `arroyo`), x86_64 only — the default build never links `drivers/xhci/irqstorage.rs`, so
  the staged path is **byte-identical**. The live path is per-descriptor + coherent: it fires only for
  in-place-only staged descriptors (`FILE_OPNAME==0`) with a mounted volume + the service task up;
  growable/created descriptors and the no-FAT core fall through unchanged.
- **Tested (QEMU):** `./arroyo check` both arches, knob on + off. Knob-ON `test 40` (non-FAT) = 18
  PASS + MISSION + `bx-blockreq` PASS. Knob-ON `UNAOS_FATIMG=sf ./arroyo test 150` = **23 PASS** (22
  fixtures + the S3 synchronous-write witness), with HELLO.BIN read + SCRATCH.BIN write/read-back
  served LIVE. Knob-OFF `test 40` = 18 PASS + MISSION, `UNAOS_FATIMG=sf test` = 22 PASS,
  **byte-identical** (no STOR-1 lines). `UNAOS_NOSTORAGE` clean both knob states. Lane: new
  `drivers/xhci/irqstorage.rs` + `arch/x86_64/syscall.rs` + `builder`/`arroyo` + `main.rs`; `fat.rs`/
  `block.rs` reused unchanged; **zero aarch64 files**.
- **✅ Metal (2026-07-10 attended bench, real 2012 rMBP):** clean full-chain usbdebug boot over FTDI
  serial — `:: bx-blockreq: PASS ::` (a raw LBA-0 read through the SERVICE TASK matched the polled read →
  **transfer-IRQ I/O + the submit/block/complete handshake work on the real Panther Point xHCI**,
  resolving design risk 1 — the one genuinely unproven thing), `U6bx` HELLO.BIN read LIVE, `S3`
  synchronous write-through + `U9x` SCRATCH.BIN write/read-back LIVE on the real FAT16 card, and the
  entire prior chain U1a→U6gx re-confirmed knob-ON; zero faults/timeouts/deadlock, HELLO.BIN intact.
  Off by default still (the knob is the metal caveat). The bench ran pre-fix code; the review fixes are
  transparent to the witnessed paths, so the confirm stands, and the fixed code re-benches at S4.
- **Review (security-arc, 2 must-fix + 3 notes, all folded before merge):** MF1 — every `REQ_QUEUE`
  hold is IRQ-masked at all 3 sites (closes a same-core service-task-preemption deadlock; masking both
  sides avoids the swap-the-trap trap). MF2 — `sys_open` refuses a writable open of HELLO.BIN
  (`rw && sidx==0 → -EACCES`), modeling immutable EL0 code as read-only — the root-cause close for the
  S3 write-through overwriting the on-disk executable (honest severity: feature-gated code-image
  regression, not a live ring-3 exploit). N1 offset-over-advance ledgered (rides S4), N2 `submit`
  panics off-scheduler, N3 docs-root paths fixed.
- **S4 (synchronous grow/create/delete)** stays a deliberately-deferred follow-on (design decision 4;
  its cross-process races are metal-only, per risk 3).
- **⚠ test-harness fact:** `./arroyo test-fat sf` is INTERMITTENTLY flaky — the OVMF USB-touch (builder
  ~line 226) sometimes makes the kernel misread the usb-storage geometry as 64 MiB (usb.img size) →
  `parse_bpb` rejects it → `NotFat` → fixtures run in-memory (18 PASS, looks like a regression but
  isn't). `UNAOS_FATIMG=sf ./arroyo test 150` (env at script start) is the RELIABLE FAT form.

## hw-jetson track — 2026-07-10 (JD4 — read-side navigation + last dead levers + screen-on-boot; same-day attended bench)

### JD4 — `ls <dir>` / `cd` / `pwd` / `cat <path>` on the panel shell + JB2c/JB9b lever retirement + screen-on-boot ✅ METAL-CONFIRMED (2026-07-10) `hw-jetson`
- **What (M1, arch-neutral, `5ca6e28`):** the shell grows a working directory and path-aware file
  commands on the read-only FAT walkers. One seat-granted additive `fat.rs` helper —
  `pub read_dir(first_cluster)` (0 = root, the `..`-to-root convention; read-only, NO lock; F3 may
  revisit read-side locking) — everything else in `shell.rs`: cwd as a normalized CANONICAL absolute
  path string, re-resolved from the root each command (a swapped card can never leave a stale chain
  head), lexical `.`/`..` normalization, case-insensitive 8.3 component walk, errno-tagged honest
  errors (`-ENOENT`/`-ENOTDIR`/`-EISDIR`/`-EIO` printed, never swallowed). WRITE path deliberately
  deferred to JD5 (pi4 F3 is about to churn `fat.rs` and a write path wants F3's locks anyway).
- **What (M2, behaviour-neutral, `436d7ef`):** retire the four dead levers JD3 left —
  `jb2c_padctl_powerup`/`jb2c_usb2_trk_clk` + `jb9b_ao_sid_fix`/`jb9b_accept_bypass_sid` (all dead
  behind the inherit gates `JB9H_SKIP_CHAIN`/`JB9_PROBE`) + their orphaned private helpers, −313
  lines. The ⭐ JB9 recipe and BOTH firmware-destroying-lever compile-asserts stay untouched.
- **What (M3, tegra-only, `195ab88`):** screen-on-boot — the panel console appears at the first
  keystroke OR a ~8 s CNTPCT wall-clock deadline (the JD3 timerless idiom), so a panel boot ends at
  a visible prompt instead of waiting for a blind keystroke. Headless boots unchanged (JB2b serial
  evidence contract holds).
- **Tested (QEMU):** `check` + `UNAOS_TEGRA=1 check` green both arches; `UNAOS_HUBSTORAGE=1 test 25`
  MISSION (shared shell/fat guard); `test-arm 22` MISSION; `UNAOS_GICV3=1 test-arm 40` CAPSTONE 6/6;
  `esp-jetson` links, **108 `tegra:` strings** (validate media by count, not size).
- **Metal — ✅ PASS (2026-07-10 attended bench, serial `jetson-serial-2026-07-10-135751.log`):**
  screen-on-boot fired 3/3 boots (no key, ~8 s, post-CAPSTONE); zero `JB2c`/`JB9b` lines any boot;
  full navigation sequence on a **FAT16** card (`UNAOSRW` + fresh `DOCS/README.TXT`, Alcor reader
  slot 5) — `diskinfo`/`ls`/`cd docs`/`ls`/`cat readme.txt`/`pwd`/`cd ..`/`cat /docs/readme.txt` +
  `-ENOENT`/`-EISDIR` probes, all typed lowercase (case-insensitive 8.3 proven). Boots 1–2
  reconfirmed the JD3 hub-MSC intermittency (`vid=0000` on route 0x4) with a GRACEFUL settle-window
  fallthrough each time; the tegra bench pattern is a separate data card, not the boot stick.
- **Detail:** [`arch_arm64.md` §JD4](dev/OS/01_BOOT_HAL/arch_arm64.md). **Commits:** `436d7ef` M2 ·
  `5ca6e28` M1 · `195ab88` M3 (`hw-jetson`).

## ux track — 2026-07-10

### TSTE-1 — `tste`: the in-OS self-test suite (x86 + aarch64) 🔬 `ux-tste`
- **What:** running tests no longer needs a host. From a booted shell (x86 GUI, the Orin panel, or
  the serial console) `tste` runs the suite and prints ONE three-section PASS/FAIL/SKIP table:
  **[boot-time]** replays the boot-sequenced fixture verdicts, **[live]** re-runs everything that can
  honestly re-run post-boot, **[skipped]** lists what can't (with reasons). New
  `crates/kernel/src/selftest.rs` owns the suite; `shell.rs` gains a `tste` arm only. `tste` prints
  in the console like `ps` (it does NOT take the screen) and is READ-ONLY toward storage. Every line
  mirrors to serial as `:: TSTE: <name> -> PASS/FAIL/SKIP ::` + a `:: TSTE: N pass M fail K skip
  (+B boot) ::` summary (the QEMU gate).
- **[live] registry:** `sched.introspection` (meter counters readable + monotonic, ≥1 CPU);
  `heap.roundtrip` (alloc/free/realloc round-trip); `video.geometry` (draw_line/fill_triangle
  verified on an OFFSCREEN GneissPal buffer via the trait-default rasteriser — zero touch to the
  visible framebuffer); the six sync primitives `sync.{mutex,rwlock,semaphore,channel,condvar,join}`
  re-verified ON DEMAND with FRESH worker tasks (the coordinator never blocks — on the unscheduled
  x86 BSP a blocking primitive would panic/bail, so all blocking runs in spawned workers it only
  polls with a bounded budget; a broken/hung primitive surfaces as FAIL/SKIP, never a shell hang);
  `storage.{mount,rootwalk,readfile}` (FAT mount, root walk, HELLO.BIN length+content — read-only).
- **M2b boot-verdict replay:** a fixed 64-entry static ring in `selftest.rs` captures every
  `-> PASS`/`-> FAIL` fixture line as it is emitted, via ONE additive hook at the serial `_print`
  seam (both arches) — alloc-free, `try_lock` only, safe from IRQ-masked print contexts, zero change
  to what is printed. This is what lets `tste` replay the boot-sequenced fixtures it cannot itself
  re-execute.
- **SKIP-honesty:** the sync section SKIPs (single core / probe timeout) and the storage section
  SKIPs (no FAT volume) rather than faking a result. The full CROSS-CORE CAPSTONE stress + the EL0
  U-arc fixtures stay boot-sequenced; re-running them on demand needs a launcher refactor — the
  **TSTE-2** horizon, stated in `tste`'s own footer.
- **Tested — QEMU:** `./arroyo check` (x86 + aarch64) and `UNAOS_TEGRA=1 ./arroyo check` green;
  `./arroyo test 25` and `./arroyo test-arm 22` both **MISSION SUCCESS** (boot unperturbed — `tste`
  runs only on command). Live demo via QMP `send-key` to the usb-kbd (`scripts/qmp_type.py`): typing
  `tste` produced 16 boot-replayed + 9 live PASS (`9 pass 0 fail 0 skip (+16 boot)`); under
  `UNAOS_FATIMG=sf` the storage checks PASS too (`12 pass 0 fail 0 skip (+19 boot)`), all six sync
  primitives PASS on real QEMU. Metal/panel verdict rides `./arroyo x86` and the next Orin bench.

## gfx track — 2026-07-10

### VUG-1 — the crystal: `vug` becomes the graphics engine's living demo (x86 + aarch64) 🔬 `gfx-vug`
- **What:** rebuilt `vug` from a static test pattern into a real-time, software-rendered rotating
  quartz crystal (an elongated hexagonal bipyramid — 14 vertices, 24 triangles), drawn through the
  Gneiss PAL. Arch-neutral and float-free: geometry, a two-axis tumble, and perspective projection
  run in Q16.16 fixed point off a 256-entry brad sine table. Solid mode does backface culling
  (screen winding) + painter's-order depth sort + per-face integer Lambert shading (deep amethyst on
  the #1E1E1E Can-Am grey, lilac seam highlights); `vug wire` shows the wireframe; `vug bebox` keeps
  a BeBox tribute; any key exits cleanly back to the shell (the `took_screen` contract). This starts
  the **graphics-engine ledger** — see [`dev/OS/08_VIDEO/engine.md`](dev/OS/08_VIDEO/engine.md).
- **Engine primitives added (additive; fbcon/Console/Screen contracts unchanged):** `draw_line`
  (Bresenham) and `fill_triangle` (scanline) on FrameBuffer/Screen/PAL with damage tracking;
  `pal::pump_and_poll` so a full-screen interactive loop drives its own input (xHCI HID + aarch64
  UART) and exits on a key.
- **M3b corner meters:** a RENDER meter (the honest software "GPU monitor" — per-frame render/present
  cycle span vs whole-frame span => busy %, frame time / FPS, drawn triangles + estimated filled
  pixels) and a BeOS-Pulse-style **CPU pulse** meter (per-core busy/idle from additive relaxed
  lock-free counters bumped at the dispatch/idle points of both schedulers; read via
  `sched::meter_cpu_count`/`meter_cpu_ticks` — introspection only, never on a scheduling path). Both
  render through the damage-tracked back buffer; one present per frame holds. Each meter names the
  seam a real GPU/PMU utilization feed would replace.
- **Tested — QEMU:** `./arroyo check` (x86 + aarch64) and `UNAOS_TEGRA=1 ./arroyo check` green;
  `./arroyo test 25` and `./arroyo test-arm 22` both **MISSION SUCCESS** (vug runs only on command,
  so it does not perturb headless boots — the regression suites are un-regressed by the added
  scheduler counters). **Visual verdict is attended-pending:** `./arroyo x86` (QEMU GUI) and the next
  Orin bench are where the crystal spins. The `:: VUG: crystal live — 24 faces, solid/wire, exit
  clean ::` and `:: VUG: crystal exit clean — N frames ::` serial lines are emitted by `run_crystal`
  when the demo is invoked (GUI-verified-pending; headless gates never type `vug`).
## hw-jetson track — 2026-07-10 (JD3 — code arc + same-day attended bench)

### JD3 — storage behind the hub → real `ls`/`cat` on the Orin panel shell (+ dead-code retirement) ✅ METAL-CONFIRMED (2026-07-10) `hw-jetson`
- **What:** JD2's panel shell had no disk behind its `ls`/`cat`. JD3 brings up the hubbed
  mass-storage device (the Alcor reader) and gives the shell real files. The shell↔FAT↔block wiring
  is already architecture-neutral, so the work was *when/how* the tegra path drives the block device:
  **(M1)** run the (previously-skipped) `service_storage` SCSI bring-up **inside the pre-drop
  `jb2b_attach` pump**, while the JM4 timer is live — the only place the BOT/control pump's bounded
  `crate::hlt()` waits have a wake source — with a bounded 8 s storage-settle window after the
  keyboard arms so a device that enumerates behind the hub can publish `BLOCK_DEVICE` before the
  drop. **(M2)** make the *post*-drop panel shell's reads work on the timerless EL1 core: the JM6
  drop disables the physical timer but left `timer::LIVE` stale-true, so `arch::hlt()` would
  WFI-park the core forever the first time `ls`/`cat` hit the BOT pump. Fix = `timer::set_not_live()`
  after the drop (→ `hlt()` busy-spins) + convert the shared `pump_until_bot_done` from a fixed
  iteration budget to a `now_cycles`/`hw_wait_budget` **wall-clock** deadline (so a busy-spinning
  `hlt()` doesn't time out in microseconds). The wall-clock change mirrors the Pi's polled EMMC2
  driver (CNTPCT-deadline reads with the timer IRQ off) and is arch-neutral. *The one shared-file
  edit (`drivers/xhci/mod.rs`) is the "xHCI seam" the JD3 brief pre-authorised the jetson track to
  request from the integrator.* **(M3)** retire the dead JB3/JB4/JB5 "revive the halted Falcon"
  machinery — dead since the JB9 inherit pivot (gated const-false, already optimizer-pruned): the
  `!jb9h_skip`/`JB5_RUN_E_REPLAY` call blocks in `main.rs` + the orphaned functions across
  `bpmp_tegra.rs`/`xusb_tegra.rs`/`smmu_tegra.rs` (~840 lines). KEPT: the ⭐ JB9 recipe, BOTH
  firmware-destroying-lever compile-asserts (+ their guard consts), the shared XUSB/MMU register
  consts, the read-only post-attach diagnostics, and the JB9 forensic kit. Behaviour-neutral.
- **Tested — QEMU:** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` green both arches;
  `UNAOS_HUBSTORAGE=1 ./arroyo test 25` → **MISSION SUCCESS** (`storage_slot=2 note='ready'`, no BOT
  timeout — the primary guard for the shared BOT-pump change); `./arroyo test-arm 22` → **MISSION
  SUCCESS** (aarch64 BOT-pump guard); `UNAOS_GICV3=1 ./arroyo test-arm 40` → **CAPSTONE 6/6**;
  `esp-jetson kernel.elf` links, **107 `tegra:` strings** (JD2 was 105).
- **Tested — ✅ METAL (2026-07-10, attended; Peter at the Orin):** an SD card in the **Alcor USB reader
  behind the hub** enumerated (`vid=058f pid=6362 … route 0x4 tier 1` → `HUB DOWNSTREAM MASS STORAGE
  (slot 5)`); M1's `service_storage` ran the SCSI bring-up pre-drop (`Disk 'Generic' 'USB SD Reader'
  … 29 MiB` → `READ(10) LBA0 … Passed` → `JD3 — mass storage ready`), and post-drop the panel shell's
  `diskinfo`/`ls`/`cat` read the FAT card — **PASS**, with **zero `BOT pump TIMEOUT`** anywhere (the M2
  timerless wall-clock BOT read + `set_not_live()` proven on silicon). First real filesystem content on
  the Orin panel. Bench note (confirms the flagged risk): the reader's hub-downstream enumeration is
  intermittent — the first boot the hubbed LS/FS devices failed and the 8 s settle window correctly fell
  through to `no mass storage … proceeding` (graceful, no wedge); a re-seat + power-cycle brought it up.
  Serial `~/unaos-bench/jetson-serial-2026-07-10-104357.log`.
- **Detail:** [`arch_arm64.md` §JD3](dev/OS/01_BOOT_HAL/arch_arm64.md). **Commit:** see `git log` (`hw-jetson`).
## hw-rmbp track — 2026-07-10 (U6gx — UnaFS owner/grants ACL, the x86 twin of pi4 U6, Opus-executed)

### U6gx — UnaFS owner/grants: by-name ACL at SYS_OPEN + SYS_FGRANT delegation + F1 owner-only unlink (x86) ✅ METAL-CONFIRMED (2026-07-10) `hw-rmbp`
- **What:** the x86 twin of the aarch64 U6 owner/grants ACL — closes the U11x M2 ledger anchor
  (cross-process open/unlink of a created file was GRANT-FREE on x86). Secure-by-DEFAULT: an
  `O_CREAT` of a NEW name records the creator as OWNER (PRIVATE); the new `O_PUBLIC` mode bit
  opts out to world-access. An open of an existing owned file is admitted only for the owner or a
  principal the owner `SYS_FGRANT`ed (a `CAP_READ|CAP_WRITE` subset), else `-EACCES`; a file with
  no owner row (a STAGED file / an `O_PUBLIC` create / a torn-down owner) is PUBLIC (pre-U6gx
  behaviour). **F1 folded from the start** (pi4 learned it post-hoc): `SYS_UNLINK` is OWNER-only
  for an owned file, so a content grantee cannot `unlink`+`O_CREAT` to steal ownership.
- **How (the x86 substitutions):** the ACL is keyed by the created file's `U10_NAMES` **name-id**
  (a direct index, not pi4's `(dir_lba,dir_off)` — no recycled-key aliasing, no bounded-table
  "full", so `owned_set_owner` is infallible with no fail-closed path); the PRINCIPAL is the
  address-space SLOT fenced by `SLOT_GEN` (the `(ASID, ASID_GEN)` analogue). `OWNED_FILES` is a
  `SpinMutex<[OwnedFile; N_U10_NAMES]>` taken IRQ-masked via `without_interrupts` (syscall + the
  teardown path symmetric — the pi4 `IrqGuard` discipline); cleared at unlink + owner teardown
  (`owned_clear_owner_slot` in `clear_handle_row`, reverting to public). `SYS_FGRANT = 18` names
  the grantee OWNER-SCOPED by a `Child` handle (the `SYS_XFER` idiom); ownership is checked before
  the grantee handle is resolved. Additive on `arch/x86_64/syscall.rs`; **zero aarch64 files**.
- **Tested:** a two-process `u6gx_launcher` (owner A + grantee B on their own cores, GO/SIG
  choreography) witnesses the full matrix incl. the U11x M2 combined path (owner unlink while B
  holds open → deferred; re-create `-EBUSY`; ownership dies at B's last close): `:: U6gx: UnaFS
  owner/grants — non-owner open -EACCES, owner grant admits R|W (content crossed), non-owner grant
  -EACCES, grantee unlink -EACCES (delete owner-only), revoke re-denies, owner unlink defers while
  grantee holds + re-create -EBUSY, ownership dies at last close -> PASS ::`. `./arroyo check` both
  arches; `test 40`/`90` (in-memory core) 17 PASS + U6gx; `test-fat sf` + `p16` **22 PASS 0 FAIL**
  each (21 priors byte-identical + U6gx; `grep 'PASS ::'` reads 21 — it misses U2-0a's `PASS (...)`
  format); `UNAOS_NOSTORAGE=1 test 90` clean skip. Metal: pure syscall logic, storage-gated
  (metal-pending like U8x/U11x — flash-and-watch at the next attended bench).
- **✅ METAL (2026-07-10 attended bench, mid-2012 rMBP, boot 2 of 3):** the FULL x86 chain
  U1→U11m2→U6gx confirmed on silicon in one boot — 23 PASS, zero FAIL: U9x/U10x staged
  writes/grow/create/delete flushed on the real card, U11m2 delete HELD past unlink + released at
  last close AND teardown + DELETED on FAT (chain freed all copies), the complete U6gx
  owner/grants matrix. Bench facts: boot 1 hit 3 `BOT pump TIMEOUT`s (USBSTS PCD set — the
  Generic USB SD reader bounced mid-battery, same intermittent family as the jetson Alcor; clean
  boot 2 had zero) — the BOT layer still has NO endpoint-recovery path if a device stalls
  mid-battery (honest gap; the STOR-D1 ladder's natural home). Boot-1 also proved fixture-arch
  contamination is real: an aarch64 HELLO.BIN on the shared data card fails U2/U4x/U6x with
  "never returned" signatures — keep per-arch HELLO.BIN on bench cards. Boot 3: GUI build
  attended — `tste` three-section table + the vug crystal on the retina panel (vug present rate
  is uncached-GOP-VRAM-bound; a PAT/write-combining arc is PARKED as a candidate).
- **Commit:** see `git log` (`hw-rmbp`).

## hw-pi4 track — 2026-07-10

> **⭐ METAL BENCH (2026-07-10, attended — real Pi 4, Debug Probe serial):** the entire FAT-mutating
> stack confirmed on silicon in one boot off the real EMMC2 card (`@0xfe340000`, 15193 MiB CSD v2):
> **23 PASS** — U9 in-place write, U10 grow/create/delete (real cluster allocation + both-FAT mirror +
> directory RMW on the card), U11 M1/M2/M2b (close, unlink-defers-free, and the reaper freeing a real
> teardown-orphaned chain), and **U6-grants** (owner ACL + the F1 grantee-unlink denial witnessed on
> metal) — plus CAPSTONE 6/6, the 3 expected M6b kills, no leaks/faults, and **zero `M6g: R1 error`
> lines** (the new R1 check rode the whole battery with no false positives on a healthy card).
> Bench note: fixtures are stateful — a battery boot mutates `GROW.BIN`/`SCRATCH.BIN` and creates
> `OWNED.BIN`/`FRESH.BIN`/`B11.BIN`; re-runs on a stale card self-report `(stale image) — demo skipped`
> and are NOT failures. Re-prep = restore pristine `GROW.BIN`+`SCRATCH.BIN`, delete the created files.
> One ancient 31 MB SD-1.0 card (`UNAOSRW`) was refused by the Pi 4 EEPROM bootloader outright
> (no firmware output at all) — prefer the known-good 16 GB card.

### F2 — SMP-hardening of the FAT-mutation seams (aarch64) ✅ METAL-CONFIRMED (2026-07-10) `hw-pi4`
- **What:** the whole FAT-mutating stack is metal-confirmed, but the U11-M2b reaper made a SECOND
  cross-core FAT writer permanent while two single-core-only assumptions remained. This arc closes the
  ones in the pi4 lane before SMP scheduling widens (not live today — the reaper's free is
  await-verdict-sequenced strictly after all writers exit).
  - **M1 — `fat::set_fat_entry` lost-update CLOSED.** The all-copies read-modify-write of a FAT sector
    was unserialized: two cores mutating entries in the same sector (or the two mirrored FAT copies)
    could read-before-the-other's-write and clobber an update. A new aarch64-only `FAT_MUTATION`
    spinlock (`fs/fat.rs`), acquired IRQ-masked via `arch::without_interrupts` (non-preemptible → a
    proper IRQ-safe spinlock at any core count, the reaper's `IrqGuard` discipline), serializes the whole
    RMW. Span is bounded to a single FAT-sector RMW — never a free-search / data loop / `mount()` — safe
    because the aarch64 storage path is polled. aarch64-only: x86's FAT path `hlt`s awaiting an async
    xHCI event, so masking IRQs across it would hang (`with_fat_lock` is a zero-cost passthrough there,
    x86 byte-identical). `5645123`.
  - **M2 — the `sys_fgrant` grantee (asid,gen) TOCTOU CLOSED** (`arch/aarch64/syscall.rs`). The grant
    captured the grantee identity as two separate atomic loads; a child exiting + ASID-recycling between
    them could bind the grant to the recycled incarnation (misdelegation → privilege escalation /
    disclosure). Now re-validates `state==PRUNNING && pid && asid` after the gen read (an ASID's gen bumps
    only at teardown, which flips those) → `-ECHILD` on any mismatch. This was surfaced by a 23-agent
    adversarial audit of the U6 (`OWNED_FILES`) + U11 (`OPEN_FILES` / open-vs-unlink) seams. `10e3c65`.
  - **M3 — cross-core witness (with teeth).** An in-RAM stress on the SAME lock (no on-disk FAT touched,
    zero volume risk): a joinable worker on `demo_cpu` + this task's half inline race a non-atomic
    counter. On 4-core QEMU raspi4b (round-robin TCG) the UNLOCKED control raced away **~48%
    (116001/240000)** of its increments while the LOCKED path lost **NONE** — a genuine cross-core
    demonstration that the race is real AND the lock closes it. Emits an `F2-witness:` line, deliberately
    not a `-> PASS` line, so the 23-fixture count stays byte-equivalent. `55451da`.
  - **Audited SOUND / ledgered:** the U11 refcount lifecycle + the U6 owner-check are SMP-sound (verified,
    no change); two windows are benign-by-design. The remaining races are metal-latent
    (excluded-by-sequencing) and ledgered with identified fixes in [`SECURITY.md`](SECURITY.md) — most
    notably the F2 flag's OTHER named leg, `alloc_cluster`'s free-search-then-claim **cluster-aliasing**
    (SIMPLE, compare-and-claim under `FAT_MUTATION`, deferred to avoid restructuring the metal-confirmed
    allocator beyond M1's `set_fat_entry` scope), the directory-sector RMW twin, and the
    open-races-unlink / recycled-slot-ownership races that need a broader UnaFS namespace lock.
- **Tested:** `./arroyo kernel8-test` — **23 PASS** (byte-equivalent) + CAPSTONE 6/6, only the 3 expected
  M6b kills, no leaks/faults, zero R1/CMD13 error lines, **+ the `F2-witness:` line** (locked 240000/240000
  intact, unlocked lost 116001/240000). `./arroyo check` both arches; `./arroyo test-arm` MISSION SUCCESS.
  ✅ **METAL-CONFIRMED (2026-07-10, attended bench, real Pi 4 / EMMC2 card):** the full 23-PASS battery ran
  through the `FAT_MUTATION`-serialized `set_fat_entry` RMW on silicon (so M1's on-disk RMW is metal-verified),
  CAPSTONE 6/6 (all 4 cores up), zero R1/CMD13 errors, only the 3 expected M6b kills; and the F2-witness held
  under TRUE parallelism + preemption — **LOCKED 240000/240000 intact, UNLOCKED control lost 119998/240000
  (~50%, more contended than QEMU's ~48%)**. Serial: `~/unaos-bench/pi-serial-2026-07-10-110417.log`.
- **Lane:** `fs/fat.rs` (aarch64 path — seat-granted for this arc) + `arch/aarch64/syscall.rs`; **zero x86
  behavioural change** (all new code cfg-gated `target_arch = "aarch64"`).

### F3 — the UnaFS namespace / FS-metadata lock (aarch64) — every F2-ledgered race CLOSED ✅ METAL-CONFIRMED (2026-07-10) `hw-pi4`
- **What:** F2 serialized the single FAT-sector RMW and ledgered six residual concurrent-FS-mutation
  races (all excluded-by-sequencing today). F3 closes all six, same discipline: behaviorally-transparent
  single-core (23-PASS battery byte-equivalent), cfg-gated aarch64 locks, zero x86 OBSERVABLE-behaviour
  change (the shared `alloc_cluster` body's on-disk write ORDERING changed on x86 too — claim before
  zero-fill; end-state identical, crash-window ordering noted in SECURITY.md).
  - **M1 — `alloc_cluster` compare-and-claim** (the F2 flag's last leg, cluster-aliasing CLOSED). The
    free search stays unlocked; the CLAIM re-reads the candidate entry under `FAT_MUTATION` and sets EOC
    only if still free (loser rescans — bounded retry via the new lock-free `set_fat_entry_inner`).
    Zero-fill moves AFTER the claim (zeroing an unclaimed cluster could scribble a winner's linked
    data); the cluster is EOC-reserved but unlinked during the fill, so no reader sees stale bytes; a
    zero-fail after claim orphans it (benign lost cluster), never aliases.
  - **M2 — `DIR_MUTATION`** (directory-sector RMW lost-update CLOSED). The aarch64-only IRQ-masked twin
    of `FAT_MUTATION` wraps the three directory-sector RMW bodies (`write_dir_entry_fields`,
    `mark_dir_deleted`, `create_in_root`'s slot write — never a scan). FAT + dir locks guard disjoint
    sectors and never nest.
  - **M3 — the NAMESPACE lock** (the four sequence races CLOSED: open-races-unlink stale-chain UAF,
    `owned_clear`-vs-`owned_set_owner` recycled-slot ownership theft, create slot-claim/duplicate-name,
    the `0xE5`-before-mark-pending window's open-vs-unlink half — its last-close half was already U11's
    mark-pending-before-drop under `OPEN_FILES`; `sys_close` takes no ns). One per-mount IRQ-masked SpinMutex (`NsGuard`, in
    `arch/aarch64/syscall.rs`) spans `sys_open`'s `find_located → ACL → incref/files_alloc` (incl. the
    create path) and `sys_unlink`'s `0xE5 → owned_clear → mark-pending → descriptor-drop`. Held across
    the bounded polled directory I/O BY DESIGN, but never across `mount()`/chain frees —
    `files_free_by_dir` now collects orphan heads (`#[must_use]`), freed after the guard drops. Strict
    lock order: `NAMESPACE ⊃ {FAT_MUTATION, DIR_MUTATION, OPEN_FILES, OWNED_FILES, DEFERRED_FREE}`,
    never acquired while an inner lock is held.
  - **M4 — F3-witness** (the F2 pattern): in-RAM cross-core stress of the exact `ns_lock` guard — the
    LOCKED pass loses nothing, the UNLOCKED control races away increments under QEMU RR-TCG. Emits an
    `F3-witness:` line (not a `-> PASS` line). HONEST SCOPE: the full open-vs-unlink disk-sequence
    interleave is not provokable from the single-EL0-core QEMU battery — metal-latent, rides the bench.
- **Tested:** `./arroyo kernel8-test` — **23 PASS** (byte-equivalent) + CAPSTONE 6/6, only the 3 expected
  M6b kills, no leaks/faults, zero R1/CMD13 error lines, + the `F2-witness:` AND `F3-witness:` lines.
  `./arroyo check` both arches; `./arroyo test-arm` MISSION SUCCESS.
  ✅ **METAL-CONFIRMED (2026-07-10, attended bench, real Pi 4 / EMMC2 card, post-merge tip `8757b27`):**
  the full 23-PASS battery ran through every F3-serialized path on silicon (compare-and-claim alloc,
  `DIR_MUTATION` dir RMWs, namespace-locked open/create/unlink sequences) — CAPSTONE 6/6 (all 4 cores),
  zero R1/CMD13, no leaks/faults, only the 3 expected M6b kills, and NO stall around the open/create/
  unlink fixtures (the seat's ns-span polled-I/O latency watch: clear at real card timing). **The
  F3-witness held under TRUE parallelism + preemption: LOCKED 240000/240000 intact, UNLOCKED control
  lost 120000/240000 (exactly 50%)**; the F2-witness re-held identically (120000/240000 this boot).
  Honest scope unchanged: the two-cores-mid-syscall DISK-sequence interleave stays unprovokable until
  multiple EL0 cores run FS syscalls concurrently. Serial: `~/unaos-bench/pi-serial-2026-07-10-134423.log`.
- **Lane:** `fs/fat.rs` (aarch64 path — same seat grant as F2) + `arch/aarch64/syscall.rs`; **zero x86
  behavioural change**.

### emmc2 R1-status hardening — the card's own verdict checked after CMD17/CMD24 (aarch64) ✅ METAL-CONFIRMED (2026-07-10) `hw-pi4`
- **What:** `drivers/emmc2.rs`'s polled CMD17 (READ_SINGLE_BLOCK) and CMD24 (WRITE_SINGLE_BLOCK)
  issued the command and moved the data but ignored the card's **R1 status word** — the controller's
  interrupt error bits cover only link-level failures (CRC/timeout/index), so a card-REPORTED error
  (address out of range, write-protect violation, ECC failure, card-locked, …) was silently swallowed
  and a bad read/write looked like success. Both paths now check RESP0 against the SD Physical Layer
  §4.10.1 error mask (`R1_ERROR_MASK = 0xFFF9_8008`) immediately after the command completes, before
  touching the PIO FIFO — symmetric across the two commands (the U9 mirror discipline) — logging one
  `:: M6g: CMDnn R1 error status 0x… ::` diagnostic line and surfacing `BlockError::Io` up through
  `drivers::block` instead of returning fabricated success.
- **Tested:** `./arroyo kernel8-test 35` — **23 PASS** (all prior verdicts present incl. the full
  U9/U10/U11/U6 FAT-mutating battery), CAPSTONE 6/6, only the 3 expected M6b kills, no leaks, and no
  R1-error lines (QEMU's SD model returns clean R1, so the healthy path is proven un-regressed);
  `./arroyo check` both arches green; `./arroyo test-arm 30` MISSION SUCCESS. The error leg itself is
  unreachable under QEMU (its SD model doesn't fabricate card errors) — it is metal-relevant hardening,
  exercised on silicon only when a real card complains.
- **Commit:** see `git log` (`hw-pi4`).

### emmc2 CMD13 post-write status — programming-phase errors surfaced (aarch64) `hw-pi4`
- **What:** completes the R1 arc's story for writes. The R1 check catches a card that REJECTS CMD24,
  but a card can also fail while PROGRAMMING the block after a clean transfer (DAT0 busy phase) — and
  those errors (CARD_ECC_FAILED, generic ERROR, WP_ERASE_SKIP) appear only in a later SEND_STATUS, so
  a discarded write still looked durable. `write_block_512` now waits out programming-busy
  (`ST_DAT_INHIBIT` clear, dedicated 500 ms bound = 2× the spec's 250 ms write timeout — the plain
  100 ms command timeout would flag a slow-but-legal write), then issues CMD13 SEND_STATUS (RCA
  captured at CMD3, carried on `SdCard`) and applies the same `R1_ERROR_MASK` check — error →
  `:: M6g: CMD13 R1 error status … ::` + `BlockError::Io`. Reads are unchanged (no programming phase).
- **Tested:** `./arroyo kernel8-test 35` — **23 PASS**, CAPSTONE 6/6, no R1/CMD13 error lines, no
  busy-timeouts (every write in the U9→U11/U6 FAT battery now exits through the CMD13 check — the
  healthy path is exercised on every single write); `check` both arches; `test-arm 30` MISSION
  SUCCESS. The error leg is metal-only (QEMU never fails programming), same honest scope as R1.
- **Commit:** see `git log` (`hw-pi4`).

## hw-pi4 track — 2026-07-08 (Opus round)

### U6 — UnaFS owner/grants: the by-NAME namespace ACL, enforced at `SYS_OPEN` (aarch64) ✅ METAL-CONFIRMED (2026-07-10) `hw-pi4`
- **What:** closes the LAST documented capability gap the U6b→U11 line left. `SYS_OPEN`/`O_CREAT`/`SYS_UNLINK`
  were gated on the HANDLE capability, but the by-NAME namespace itself was NOT ACL'd — any process could
  open/create/unlink any name. This lands the in-kernel enforcement SEAM (the on-disk UnaFS `owner`/`grants:*`
  attributes will feed it once K2/K3/K4 land; no on-disk owner format exists yet and `fat.rs` is shared/off-lane).
  **Secure-by-default (owned-by-default):** an `O_CREAT` of a NEW name records the creating principal as the
  file's OWNER (PRIVATE); the new `O_PUBLIC` mode bit (bit2) opts a create OUT to world-access. An open of an
  EXISTING owned file is admitted only for the owner or a principal the owner GRANTED; else `-EACCES`. A file with
  NO owner row (pre-existing / host-created / `O_PUBLIC`) is PUBLIC — byte-identical to the pre-U6 behaviour.
  - **M1 — the `SYS_OPEN` ACL** (`21baee8`): `OWNED_FILES`, a bounded `SpinMutex<[OwnedFile;16]>` keyed by the FAT
    file-identity `(dir_lba, dir_off)` (the `OPEN_FILES` idiom); owner + up to 4 grants, ALL fenced by the
    `(ASID, ASID_GEN)` incarnation (the recycle fence the file-id/xfer/derivation machinery already uses). Checked
    in `sys_open`'s "nothing-claimed-yet" window (a clean `-EACCES`). A private create that cannot record an owner
    FAILS CLOSED (`-ENOSPC`, undoing the fresh dir entry). The row clears at unlink (FAT may recycle the slot) and
    at OWNER teardown (`clear_handle_row` — the file reverts to PUBLIC, keeping the bounded table self-cleaning);
    its lock is IRQ-masked via the M2b `IrqGuard` (acquired in BOTH syscall and teardown contexts). The two
    cross-process U11 fixtures tag their shared file `O_PUBLIC` (one mode bit each) so B still opens A's file.
  - **M2 — `SYS_FGRANT` delegation** (`8034d0c`): `SYS_FGRANT(file_handle, child_handle, rights)` (= 18). The owner
    grants (a `CAP_READ|CAP_WRITE` subset) or revokes (`rights = 0`) access to a principal named OWNER-SCOPED by a
    `Child` handle it holds (the `SYS_XFER` idiom — no raw pid/ASID from EL0). Ownership is checked BEFORE the
    grantee is resolved (a non-owner is `-EACCES` regardless of argument). The grant is an ACL edge on the FILE —
    nothing lands in the grantee's table; it opens the name and the ACL admits it; a handle it ALREADY holds
    survives a revoke (the ACL gates ACQUISITION, not held caps).
  - **F1 — delete is OWNER-only** (`1ca2e89`, an adversarial-self-review fix): `SYS_UNLINK` is gated on OWNERSHIP,
    not merely the handle's `CAP_WRITE`. A content `CAP_WRITE` grantee (which legitimately opened the file RW)
    could otherwise `unlink` + `O_CREAT` the name to STEAL ownership and lock the real owner out — the M2 gate
    missed it because the demo granted only `CAP_READ`. Now an OWNED file is unlinkable only by its owner; a
    PUBLIC file keeps the prior `CAP_WRITE`-gated unlink (so u10delete / u11defer / u11reuse / u11reap stay
    byte-identical). Two latent hardenings folded in: an ASID-0 create is PUBLIC (ASID 0 is never torn down / gen-
    bumped, so it cannot be a gen-fenced private owner), and `SYS_FGRANT` returns `-EINVAL` for a nonzero rights
    request that names only unsupported bits (instead of silently coercing it to a revoke).
- **Tested — QEMU:** `./arroyo kernel8-test 35` → one new verdict after U11-reap:
  `:: U6-grants: owner/grants on open — non-owner -EACCES, owner grant admits R|W, grantee unlink -EACCES (delete
  owner-only), non-owner grant -EACCES, revoke re-denies -> PASS ::`. A two-process fixture (`el0-uowner-a` OWNER /
  `el0-uowner-b` GRANTEE, the u11defer choreography + the U7 proc-reserve/Child-handle pre-endow): A creates
  `OWNED.BIN` private; B (a different ASID) is DENIED (`-EACCES` — the gap closed); A `SYS_FGRANT`s B read+write →
  B opens RW + reads the matching bytes; B (a non-owner) is refused `SYS_FGRANT` (`-EACCES`) and its `SYS_UNLINK`
  is refused (`-EACCES` — delete is owner-only); A revokes → B is re-denied; A (owner) re-opens its own file
  throughout. **23 PASS** (22 prior byte-identical + U6-grants), **CAPSTONE 6/6**, only the 3 expected M6b kills,
  0 unexpected faults. `./arroyo check` both arches green; `./arroyo test-arm 30` MISSION SUCCESS; `./arroyo
  kernel8` compiles.
- **Lane:** additive on `arch/aarch64/syscall.rs` (the `OWNED_FILES` table + `sys_open` ACL + `SYS_FGRANT` + the
  uowner fixture/launcher, riding the existing `u7_launcher` chain) + docs; **NO `sched.rs` / `fat.rs` / `main.rs`
  change**; zero x86 files.
- **Honest scope (flagged, not a defect):** the owner/grants store is in-kernel, VOLATILE, boot-scoped — there are
  no persistent principals yet (the model's own documented gap), so ownership is meaningful only within a boot and
  only while the owner process lives. A persistent form (and retroactively owning pre-existing/host files) awaits
  the on-disk UnaFS attributes (K2/K3/K4). The x86 twin (U6x) and a `SYS_FGRANT` that endows a directly-usable
  handle (vs. open-by-name) remain future.
- **Metal:** 🔬 QEMU-green, metal-pending. Rides the next Pi 4 boundary alongside U10/U11 (the FAT-mutating stack).
  QEMU exercises the ACL + delegation in full (no timer/IPI needed); the metal watch-item is the same `(ASID,
  ASID_GEN)` teardown-revert path the U11 reaper walks.
- **Commit:** `21baee8` (M1) + `8034d0c` (M2) + `1ca2e89` (F1), branch `hw-pi4`.

### U11 (M2b / U12b) — the teardown-last-close REAPER: deferred-free queue + kernel reaper task (aarch64) ✅ METAL-CONFIRMED (2026-07-10) `hw-pi4`
- **What:** closes the ONE honest-scope gap M2a left open. M2a proved the cross-process defer for the
  EXPLICIT-close path (the chain frees at the last `SYS_CLOSE`), but a program that EXITS holding the last
  cross-process open of an `unlink_pending` file cannot free its chain at teardown — that runs IRQ-masked, on the
  dying task's own kernel stack, TTBR0 on the boot root, immediately before `switch_context`, where multi-sector
  polled SD I/O is illegal. M2a therefore LOGGED that as a transient lost-cluster leak (benign, until reboot).
  **M2b actually frees it**, in a block-I/O-legal context, via two additive pieces (both aarch64 `syscall.rs`):
  - **`DEFERRED_FREE`** — a bounded (`NDEFERFREE = 16`) `SpinMutex`-guarded ring of chain heads with its OWN lock
    (separate from `OPEN_FILES`, so the teardown push never contends with live open/close). `clear_files_row`'s
    last-close-of-`unlink_pending` branch now PUSHES the head (`deferred_free_push` — lock + array write + unlock,
    I/O-free, the SAFE twin of M2a's teardown decrement) instead of logging the leak. A FULL queue degrades
    honestly to the M2a log-the-leak behavior — the push NEVER blocks, spins, or does I/O.
  - **`orphan_reaper`** — a forever kernel service task spawned at BOOT via the EXISTING
    `sched::spawn(name, fn, arg, cpu)` service-task API (`main.rs`'s aarch64-baremetal service block, alongside
    `input`/`render`; **NO `sched.rs` change — the feared scheduler hook was not needed**). It drains the queue:
    pop one head UNDER the lock, RELEASE the lock, THEN `free_orphan_chain` (mount + all-FAT-copies free — block
    I/O, legal here: EL1, IRQs enabled, its own stack). It `yield_now`s when empty so it never hogs its core under
    QEMU's cooperative scheduler, and is co-located with the demo VERDICT core so the launcher's bounded
    `yield_now` poll cedes it CPU to drain deterministically.
  Freed EXACTLY once: a teardown-orphaned chain reaches the queue ONLY via `openfile_decref_at` returning
  `Some(fc)` (last-close-of-pending, the row already EMPTY) — queued once, reaped once; the explicit-close path
  frees inline and NEVER queues, so no chain is both freed inline and queued. SMP-safe (a teardown on core X
  pushes, the reaper on core Y drains — one `SpinMutex`, no torn reads, no lost/duplicated entries).
- **Tested — QEMU:** `./arroyo kernel8-test 30` → after U11-reuse, one new verdict:
  `:: U11-reap: teardown-last-close reaper — A exits holding the unlinked file open, its chain freed by the reaper
  (all FAT copies) + re-allocatable, no teardown leak -> PASS ::`. A two-process fixture (`el0-u11reap-a`/`-b`,
  the u11defer choreography): A creates + writes + holds `DEFER2.BIN` open; B opens + `SYS_UNLINK`s it (deferred —
  A holds it); A reads its ORIGINAL bytes back (chain alive), then **EXITS WITHOUT CLOSING** — so TEARDOWN is the
  last close. The launcher proves the same three checkpoints as u11defer, but CHECKPOINT-3 is a bounded
  `yield_now` poll of the FAT until the REAPER has freed the chain (all FAT copies) + it is re-allocatable
  (`first_free == f0`); the serial shows `U11-defer: reaper freed teardown-orphaned chain @cluster N` and the M2a
  `"teardown … left allocated (leak)"` line does NOT appear for `DEFER2.BIN` (reaped, not leaked). **22 PASS** (21
  prior byte-identical + U11-reap — sorted scratch-worktree baseline diff vs `d57520f`, a single appended line),
  **CAPSTONE 6/6**, only the 3 expected M6b kills, 0 unexpected faults. `./arroyo test-arm 22` green (baremetal-
  gated); `./arroyo check` both arches, `./arroyo kernel8` compiles; **zero x86 files**.
- **Lane:** aarch64 `syscall.rs` (`DEFERRED_FREE` + `orphan_reaper` + the `clear_files_row` push + the reap
  fixture/launcher) + the aarch64-baremetal service-spawn block of `main.rs` (additive, cfg-scoped) + docs; zero
  x86 files. `SpinMutex` reused from `sched.rs`; **NO `sched.rs` change** and **NO `fat.rs` change** (the reaper
  calls M2a's existing `free_chain`). 🔬 Metal-pending (pure syscall lifecycle + a kernel service task + FAT free;
  QEMU proves the reaper in full — rides the next Pi boundary alongside U10/U11-M1/M2).
- **Seat coalesce-review fix (1 confirmed should-fix, in-lane):** `DEFERRED_FREE` is a bare `spin::Mutex` acquired
  in two IRQ-ASYMMETRIC contexts — `deferred_free_push` in the IRQ-masked teardown, but `deferred_free_pop` in the
  reaper TASK body, which the metal timer PREEMPTS (task bodies run I-unmasked; `SCHED_ACTIVE` on). A timer preempt
  of the reaper while it holds the lock, then a **same-core** teardown push spinning IRQ-masked on it, deadlocks
  that core (run queues never migrate → the preempted holder never releases). Dormant in a healthy ≥2-AP boot (reaper
  on `online.get(1)` vs EL0 fixtures on `online.first()` — distinct cores) but **live in the single-AP fallback**
  (`reaper_cpu`/`vcpu`/`demo_cpu` collapse onto one AP; the pi4's 3/4-core boot variance can produce it). Fixed by a
  local RAII `IrqGuard` (save/restore DAIF — the `sched.rs` `irq_save_mask` idiom, kept local so M2b **still touches
  no `sched.rs`**) masking IRQs across **both** `deferred_free_push`/`pop`, making the hold non-preemptible → a
  proper IRQ-safe spinlock at any core count. Also folded the log nit: `free_orphan_chain` returns `bool`; the reaper
  logs "reaper freed …" only on a SUCCESSFUL free (its error/leak line covers failure). Byte-transparent under QEMU
  (cooperative — no preemption): **22 PASS**, CAPSTONE 6/6, reaper freed cluster 8, no teardown-leak line, 0 faults;
  `check` both arches; `test-arm` clean; zero x86 files. **Flagged for a future SMP-hardening arc:** the
  reaper's downstream `fat::set_fat_entry` read-modify-write is unlocked across cores (no FS-mutation lock spanning
  the RMW) — a latent lost-update/cluster-aliasing race should the OS ever run uncoordinated concurrent FAT writers;
  not live today (the reaper's free is await-verdict-sequenced after all writers exit), and its fix touches `fat.rs`
  (out of this lane). **→ Addressed by the F2 arc (2026-07-10, above):** the lost-update leg is CLOSED (`FAT_MUTATION`
  serializes the `set_fat_entry` RMW, `5645123`) and witnessed cross-core (`55451da`); the cluster-aliasing leg is
  audit-confirmed with its fix specified (compare-and-claim under `FAT_MUTATION`) and ledgered in `SECURITY.md`,
  deferred to avoid restructuring the metal-confirmed allocator beyond M1's scope.

### U11 (M2 / U12) — cross-process unlink-defers-free: a global open-file refcount + deferred chain-free (aarch64) ✅ METAL-CONFIRMED (2026-07-10) `hw-pi4`
- **What:** closes the SECOND (and last) of U10's two review notes — **cross-process unlink-while-open** (POSIX
  unlink-defers-free). U10 freed a file's cluster chain the instant `sys_unlink` ran; if ANOTHER process held the
  file open, its live descriptor kept pointing at a chain a later create+grow could first-fit-reuse — a cross-file
  read/write + information disclosure. The fix defers the chain-free to the file's LAST close across ALL processes:
  - **A global (cross-ASID) open-file refcount table** (`OPEN_FILES`, `NOPENFILE = 16` rows, joined by the on-disk
    identity `(dir_lba, dir_off)`), guarded by a single **`SpinMutex`** — the scheduler's low-level spinlock, NOT
    the sleeping `Mutex` (which yields, illegal in the IRQ-masked teardown path). One row is shared by every open
    of a file across every ASID; `refcount` counts its live descriptors. The lock is held ONLY across the short
    table mutation (±refcount, read the stashed chain head) — NEVER across `mount()`/`free_chain` block I/O.
  - **`sys_open` increments, `files_free` decrements.** The increment is BEFORE `files_alloc`, so every increment
    pairs with exactly one decrement — the descriptor's `files_free` (close/teardown) or a one-line
    `openfile_decref_at` on the `files_alloc`-full unwind — with no path where a decrement lands on a row this open
    never incremented (an SMP race the after-`files_alloc` ordering would open). A full table on a NEW identity is
    a clean **`-ENFILE` (`-23`)** with reserve/unwind claim-last on every failure path.
  - **`sys_unlink` marks + defers.** It writes the directory entry `0xE5` FIRST (`fat::mark_dir_deleted` — the
    name is GONE immediately, a re-open is `-ENOENT`), marks the refcount row `unlink_pending` + stashes the chain
    head, then drops all of the caller's descriptors naming the file (`files_free_by_dir`, each decrementing). If
    the caller is the SOLE opener, the last decrement reaches 0 and frees the chain NOW — byte-identical to U10. If
    ANOTHER process still holds it open, the refcount stays > 0 and nothing is freed: that process keeps reading
    its original bytes until its last close.
  - **`sys_close` frees at the last close.** The decrement that drops an `unlink_pending` file's refcount to 0
    returns the stashed chain head, and `sys_close`/`sys_unlink` free it (`fat::free_chain`, all FAT copies) in
    syscall context AFTER releasing the lock — block I/O is legal there.
  - **`fat::delete_located` split** into `mark_dir_deleted(dir_lba, dir_off)` (the `0xE5` write) + `free_chain(
    first_cluster)` (the all-FAT-copies chain free); `delete_located` remains their pre-validated composition for
    the immediate path (the U10 "bad chain → nothing changed" contract holds byte-for-byte).
- **Tested — QEMU:** `./arroyo kernel8-test 30` → after the U11 (M1) PASS, one new verdict:
  `:: U11-defer: cross-process unlink-defers-free — name gone at unlink, reader keeps original bytes, chain freed
  (all FAT copies) + re-allocatable at last close -> PASS ::`. A **two-process** fixture (the U7 GO-word
  choreography, launcher as the single sequencer): process A (`el0-u11defer-a`) creates + writes + reads
  `DEFER.BIN`; process B (`el0-u11defer-b`) opens + `SYS_UNLINK`s it while A holds it open. The launcher re-mounts
  the FAT at THREE checkpoints — after B's unlink the NAME is gone but the chain (cluster `f0`) is STILL allocated
  in all FAT copies; A then seeks + reads its ORIGINAL 16 bytes back (the deferred chain is alive); after A's last
  `SYS_CLOSE` the chain is FREED in all FAT copies + re-allocatable (`first_free == f0`). Both A and B drop their
  handles via explicit syscalls (B's unlink, A's close), so no teardown I/O is needed. **20 PASS** (19 prior +
  U11-defer), **CAPSTONE 6/6**, only the 3 expected M6b kills, 0 unexpected faults, no leak lines. Every prior
  verdict **byte-identical** (sorted scratch-worktree baseline diff vs `d0d12ef` — a single appended U11-defer
  PASS line; the binary-growth `VBAR`/EL0-fault-ELR shift touches only diagnostic addresses, not any verdict).
  `./arroyo test-arm 22` green (baremetal-gated); `./arroyo check` both arches; **zero x86 files** (the `fat.rs`
  split is additive — x86 never calls the new writers).
- **Adversarial review (3 lenses — refcount pairing / double-free+leak+FAT-safety / SMP+teardown lock discipline —
  with per-finding refuters).** Lock discipline, teardown-I/O-safety, SMP incref ordering, and deadlock lenses all
  clean. **1 real should-fix, caught + fixed in-arc:** the refcount row was originally keyed only on
  `(dir_lba, dir_off)`, a directory SLOT which FAT recycles for a new file the moment a delete `0xE5`'s it — so a
  file created in an unlinked-but-still-open file's reused slot could conflate the two files' refcount + deferred
  free (a lost-cluster leak / mis-triggered free; not a UAF/disclosure — refcount stayed exact). This is the M1
  descriptor-slot-reuse hole one level up, which generation tags don't cover. Fixed by (a) excluding
  `unlink_pending` rows from the open-time join (a `0xE5`'d file can't be re-opened by name, so a key match on a
  pending row is a different file → claim a fresh row) and (b) recording each descriptor's row INDEX in
  `FILE_OPENROW`, so every decrement/mark hits that exact row, never a re-searchable key. A targeted re-verify
  confirmed the fix closes it with no new leak/double-free/stale-index path.
- **Seat coalesce-review fix (a SECOND, distinct slot-recycle bug; refuted 0/3 — own follow-up commit).** The
  above fixed the incref/JOIN side; the `sys_unlink` SWEEP side had the same root: `files_free_by_dir` matched
  descriptors by the recyclable `(dir_lba, dir_off)`, so when ONE process held an unlinked file F open AND created
  a new file G in F's recycled `0xE5` slot, a `SYS_UNLINK` of G swept BOTH descriptors and both reached
  `refcount == 0`, but the loop kept only the LAST orphan chain head (`orphan = Some(fc)` overwriting) → the
  earlier chain leaked. A benign lost-cluster leak on the EXPLICIT path (refcount stayed exact, no UAF/double-free),
  distinct from the teardown-only honest-scope leak below. Fixed by freeing EVERY orphan head the sweep produces
  (`files_free_by_dir` is `sys_unlink`-only = block-I/O-legal, so it frees each inline via `free_orphan_chain`; the
  `OPEN_FILES` lock is never held across the I/O). Proven by a deterministic kernel-side check
  (`u11defer_check_double_orphan`, the `u11_check_gen_rebind` style — physically recycle a slot asserting G reused
  F's exact slot, reproduce the two-descriptor/two-pending-row state, run the real sweep, fresh-mount confirm BOTH
  chains free in all FAT copies): `:: U11-reuse: sys_unlink slot-recycle — two files sharing a recycled dir slot
  BOTH free their chains (all FAT copies) -> PASS ::`. **Now 21 PASS** (the prior 20 byte-identical + U11-reuse),
  CAPSTONE 6/6, `./arroyo check` both arches, zero x86 files.
- **Honest scope — M2a (explicit-close path) landed; the teardown-last-close gap is now CLOSED by M2b (above).**
  The cross-process defer is proven for the explicit-close path, and the teardown DECREMENT is done (a short,
  I/O-free `SpinMutex` critical section, safe in the IRQ-masked teardown context). A program that EXITS holding
  the LAST `unlink_pending` open cannot free its chain in teardown (IRQ-masked, dying stack, block I/O illegal),
  so M2a LOGGED it as a transient lost-cluster leak — now REAPED by M2b's deferred-free queue + `orphan_reaper`
  (which turned out to need NO `sched.rs` hook — the existing `sched::spawn` service-task API sufficed).
- **Lane:** aarch64 `syscall.rs` + `fs/fat.rs` (the `delete_located` split — additive, reuses `set_fat_entry`/
  `chain_clusters`) + docs; zero x86 files. `SpinMutex` reused from `sched.rs` (not reimplemented); no `sched.rs`
  change. 🔬 Metal-pending (pure syscall lifecycle + FAT free; rides the next Pi boundary alongside U10/U11-M1).

### U11 (M1) — open-file lifecycle: `SYS_CLOSE` + generation-tagged file-ids (aarch64) ✅ METAL-CONFIRMED (2026-07-10) `hw-pi4`
- **What:** gives a `File` descriptor a real end-of-life and a stable identity across slot reuse, closing the FIRST
  of U10's two review notes — the **same-process sibling-handle rebind on slot reuse**. U10 left a per-task FILES
  row as a bare `+1`-biased slot index with NO generation, so after an unlink freed a slot (`files_free_by_dir`
  invalidating a multiply-opened file's descriptors) a later `sys_open` first-fit-REUSED it and a lingering sibling
  handle silently REBOUND to the different file. The fix is the standard slotmap/generation-index:
  - **`FILE_GEN[asid][idx]`** — a per-slot generation counter, bumped on every free (`files_free` — the one free
    primitive `SYS_CLOSE`/`files_free_by_dir`/`sys_open`-unwind all route through — and `clear_files_row` at
    teardown).
  - **Packed file-ids** — the `File` handle's value word now carries `(gen << 32) | (idx + 1)` (`file_id_pack`);
    generation 0 packs to exactly `idx + 1`, byte-identical to the pre-U11 bare file-id.
  - **`file_desc_validate`** — the SINGLE seam decoding a value word to a live descriptor index (range + `FILE_USED`
    + **gen == the slot's current gen**). Every File consumer (`sys_read`/`sys_write_file`/`sys_seek`/`sys_unlink`/
    `sys_close`) funnels through it, so a stale handle to a reused slot is `-EACCES` (a gen mismatch, EVEN when
    `FILE_USED` is true again) — no rebind, and no per-syscall re-derivation (five open-coded checks → one).
  - **`SYS_CLOSE = 17`** — resolves the handle for NO right (a close is always permitted on a handle you hold),
    frees the descriptor slot (bumping the gen), and clears the handle → `0`. A `Console`/`Socket`/`Child` kind is
    `-EINVAL` (object table untouched — not closeable this arc); an unresolvable / already-closed / stale-slot
    handle is `-EBADF` (`-9`) — double-close and use-after-close are clean.
  `handle_resolve` is UNCHANGED (still returns `File(raw)`), so the U6/U9 scaffold checks that resolve File handles
  directly are byte-identically unaffected; the gen validation lives one layer out. For programs that never close,
  open→read/write→teardown is byte-identical to U10 (teardown drops the descriptor exactly as a close would).
- **Tested — QEMU:** `./arroyo kernel8-test 30` → after the U10-delete PASS, one new verdict:
  `:: U11: open-file lifecycle — SYS_CLOSE + gen-tagged file-ids: close/double-close/round-trip OK, stale sibling to
  a reused slot -EACCES (gen mismatch, no rebind), A11 unlinked + B11 present -> PASS ::`. A register-only fixture
  (`el0-u11close`, witness `0x1F`) creates + grow-writes `A11.BIN`, opens it a SECOND time (a sibling descriptor),
  `SYS_UNLINK`s via the first handle (freeing both A11 descriptors, the sibling left lingering on a freed slot),
  REUSES the freed slots by opening + writing `B11.BIN`, then proves a read through the STALE sibling is `-EACCES`
  (the slot is LIVE again for B11, so the denial can ONLY be the stale generation — not a rebind onto B11's bytes),
  and exercises `SYS_CLOSE` → `0` / double-close → `-EBADF` / close→re-open→read round-trip. A kernel-side
  `u11_check_gen_rebind` proves the mechanism in isolation on a scratch ASID (claim → mint id → free (bump) →
  re-claim the same slot → the OLD id is rejected while the slot is live, the FRESH one resolves), and a fresh
  `mount()` confirms `A11.BIN` gone + `B11.BIN` present with its content. **19 PASS** (18 prior + U11),
  **CAPSTONE 6/6**, only the 3 expected M6b kills, 0 unexpected faults. Every prior verdict **byte-identical**
  (sorted scratch-worktree baseline diff vs `ddefc40` — a single appended U11 line; no VBAR/USER_REGION shift
  surfaced in any verdict). `./arroyo test-arm 22` green (baremetal-gated); `./arroyo check` both arches; **zero
  x86 files** (the design is the pi4 lead — its x86 twin U11x is a later rmbp arc).
- **Honest scope:** M1 (generation tags + `SYS_CLOSE`) lands and closes hole 1. **Deferred to the next arc (U11 M2
  / U12):** the SECOND U10 note — **cross-process unlink-while-open** (POSIX unlink-defers-free) — needs a global
  cross-ASID `(dir_lba, dir_off)`-keyed open-file refcount + a `fat::delete_located` split. It was NOT half-landed:
  its blocker is that a program which exits WITHOUT closing must still trigger the deferred chain-free at TEARDOWN,
  and teardown (`exit` → `teardown_user_slot` → `clear_handle_row`) runs **IRQ-masked, on the dying task's kernel
  stack, TTBR0 already repointed to the boot root, right before the context-switch away** — doing multi-sector
  polled SD I/O there (plus a new global Mutex-guarded refcount table + SMP-safety proof) is arc-deep and likely
  wants a reaper, not inline teardown I/O. The `O_CREAT` ambient-namespace gap stays the future UnaFS ACL arc.
- **Lane:** aarch64 `syscall.rs` only — no `fat.rs`/`block.rs`/`arroyo` change (the fixture creates its own files).
- **Metal:** 🔬 QEMU-green, **metal pending** — pure syscall lifecycle (the generation tag + `SYS_CLOSE` are
  QEMU-provable in full); rides the next Pi 4 boundary alongside the still-pending U10 metal-verify.
- **Commit:** on `hw-pi4` (Opus-executed) — see git log.

### U10 — file GROWTH + CREATE + DELETE: cluster allocation, FAT-chain extension, directory mutation (aarch64) ✅ METAL-CONFIRMED (2026-07-10) `hw-pi4`
- **What:** gives `fat::write_at` (U9's in-place-only writer) an ALLOCATOR, so `CAP_WRITE` can now **extend and
  create** files — the three restrictions U9 kept (never write the FAT, never allocate, never touch a directory)
  are lifted in rising order of risk. New `fat.rs` primitives, all FAT-safety-critical:
  - **`set_fat_entry`** — writes EVERY FAT copy (`num_fats`, 2 on this card) at the mirrored offset, read-modify-
    write per copy (neighbours preserved), FAT32 reserved high-nibble preserved. A one-FAT write is a corrupt
    volume, so it always mirrors.
  - **`alloc_cluster`** — a bounded first-fit free search over `[2, count+2)`; **zero-fills the cluster BEFORE it
    can join a chain** (no stale-byte information disclosure), marks it EOC in all copies; `-ENOSPC` (new
    `FatError::NoSpace`) when full. Never spins, never returns a reserved/bad/out-of-range cluster.
  - **`find_located` / `write_dir_entry_fields`** — locate a directory entry's on-disk `(LBA, slot)` and RMW its
    `first_cluster`/`size`, so the reader's source of truth (dir `size`) is bumped **last**.
  - **`write_grow`** — walk chain → alloc+zero+link new clusters → RMW the data → publish dir `size` LAST. A
    crash before the last step leaves the OLD smaller size on disk, never a size claiming unwritten clusters.
  - **`create_in_root`** — format an 8.3 name (`format_83`), find a free directory slot (`0x00`/`0xE5`), write a
    fresh 0-length entry; the first grow-write allocates its first cluster. Root-dir full → `-ENOSPC`.
  - **`delete_located`** — crash-safe order: mark the dir entry `0xE5` FIRST, then free the whole chain (all FAT
    copies); a crash mid-delete leaves lost clusters (benign), never a live entry aliasing freed clusters.
  A refactor keeps the read path honest: `fat_entry` delegates to the new `pub fat_entry_copy` (one FAT-offset
  site), and `scan_dir_sector` shares the on-disk 8.3 parse with the new locator via `classify_dir_slot`/`DirSlot`
  (single source of truth). **Syscall surface:** the per-task FILES descriptor gains the dir `(LBA, off)` (captured
  at `sys_open`); `sys_write_file` splits — `len <= bytes-to-EOF` keeps U9's in-place path byte-identical, a write
  past EOF routes to `write_grow` (capped `GROW_WRITE_MAX = 8 KiB`); `SYS_OPEN` gains **`O_CREAT`** (mode bit1,
  endows RW); **`SYS_UNLINK = 16`** deletes via a File+`CAP_WRITE` handle. Grow, create, AND delete are all
  reachable ONLY through `sys_write`'s single `handle_resolve(asid, fd, CAP_WRITE)` CHECK (create via an O_CREAT
  open that endows `CAP_WRITE`), so an RO-opened / revoked (U7/U8 walk inside `handle_resolve`) / wrong-kind handle
  is `-EACCES` and can never mutate the volume — no new enforcement code.
- **Tested — QEMU:** `./arroyo kernel8-test 30` → after the U9 PASS, three new verdicts:
  `:: U10: file growth — … on-disk size grew + appended data present + both FAT copies consistent -> PASS ::`,
  `:: U10-create: file create — … on-disk entry present with right size + content, no duplicate -> PASS ::`,
  `:: U10-delete: file delete — … on-disk entry gone + chain freed (all FAT copies) + cluster re-allocatable -> PASS ::`.
  Three register-only single-slot fixtures (`el0-u10{grow,create,delete}`, witnesses `0x1F`/`0xF`/`0x1F`) drive the
  three ops through the syscall surface; each launcher folds the kernel-side proof — a **fresh `mount()` re-read**
  shows the on-disk `size` grew / the entry appeared / the entry vanished, the appended-or-written bytes are on the
  card, the original clusters survived, both FAT copies agree along the chain, and a deleted file's cluster is free
  again (first-fit re-allocatable). The delete fixture also opens its file TWICE and proves that after unlinking via
  one handle a read through the SIBLING handle is `-EACCES` (no stale reference to the freed chain). **18 PASS** (15 prior + the 3 U10), **CAPSTONE 6/6**, only the 3 expected M6b
  EL0 kills, 0 unexpected faults. Every prior verdict **byte-identical** (sorted scratch-worktree baseline diff vs
  `ca1b765` — only the new U10 lines differ, plus the binary-growth `VBAR_EL1` shift `0xad800`→`0xb1000` and the
  identity-mapped `USER_REGION` address shift it drags along, which move the M6b EL0-fault diagnostic ELR/FAR — the
  M6b **verdict** line is byte-identical). `./arroyo test-arm 22` green (the module is baremetal-gated); `./arroyo
  check` both arches; **zero x86 files** (the new FAT writers are additive — x86 never calls them).
- **Honest scope:** grow + create + delete land. **Deferred:** subdirectories, LFN (long names), rename,
  write-back caching (every mutation is a synchronous RMW to the card), root-directory-chain extension (root-dir
  full → `-ENOSPC`), and UnaFS `owner`/`grants:*` on the namespace ops (open/create/unlink are cap-gated on the
  *handle* for read/write/delete, but the by-name namespace itself is not ACL'd yet — the future UnaFS layer).
  Deletion invalidates ALL of the caller's descriptors naming the file (a same-process double-open leaves the
  sibling fail-safe, `-EACCES`); the remaining gap is CROSS-process (a file open in another process when unlinked
  leaves that process's descriptor stale — needs a global open-file refcount / `SYS_CLOSE`, deferred). An `O_CREAT`
  that then fails to claim a kernel handle leaves a harmless 0-length entry (no kernel leak). Names are strict 8.3
  (trailing-dot / multi-dot / non-representable names → `-EINVAL`).
- **Lane:** the seat's U9 widening stands — the pi4 lane covers the shared `fat.rs`/`block.rs` + the FAT image
  builder in `arroyo` for this arc (`block.rs` untouched this arc). All FAT writers are aarch64-only by call site.
- **Metal:** 🔬 QEMU-green, **metal pending** — rides the next Pi 4 boundary. ⚠ **This is the first arc that
  ALLOCATES + MUTATES the FAT and directory on a real card — the metal-risk beyond U9's single in-place sector
  (multi-sector zero-fill, both-FAT mirrored writes, directory RMW).** The launchers' fresh-mount re-read is the
  proof; the boundary should re-copy pristine `GROW.BIN` (0xC1, 512 B) between runs, as U9 does for `SCRATCH.BIN`.
- **Commit:** on `hw-pi4` (Opus-executed) — M1 `a10c4b5`, M2 `5f1b0a3`, M3 `d02fb9c`, review fixes `dac149a`, docs `45bff90`.

## hw-rmbp track — 2026-07-08 (U9x — File writes + seek, the pi4 U9 twin, Opus-executed)

### U9x M1 — real File writes + seek on x86 as a STAGED WRITE-BACK: `SYS_SEEK`, RW `SYS_OPEN`, File+`CAP_WRITE`-routed `sys_write` into a per-descriptor in-memory buffer (x86) 🔬 `hw-rmbp`
- **What:** the x86 twin of pi4 U9 — gives U6bx's read-only `File`+`CAP_READ` its WRITE half, structurally
  faithful to the pi4 lead at the CAPABILITY layer and diverging ONLY where the hardware forces it. **The
  load-bearing divergence (the same one U6bx's read side pays):** aarch64 U9 writes straight to the SD card
  INSIDE the SVC handler (its EMMC2 driver is PIO). x86 CANNOT — storage is USB-over-xHCI whose BOT pump
  `hlt()`s awaiting async completion, and the SYSCALL handler runs IF-masked (SFMASK clears IF), so a `hlt`
  at IF=0 never wakes: no disk I/O in-handler. So the write is **STAGED** — the write twin of the staged read.
  (1) **`SYS_SEEK = 15`** — a near-verbatim mirror of aarch64 `sys_seek`: File + ANY of `CAP_READ|CAP_WRITE`,
  `-EINVAL` strictly past size (seeking TO size is legal), sets `FILE_OFFSET` (now settable). (2) **RW
  `SYS_OPEN`** — a mode bit in `a2` (`0` = RO/`CAP_READ`, byte-identical to U6bx; `1` = RW/`CAP_READ|CAP_WRITE`;
  backward-safe because the first-entry GPR scrub zeroes `rdx`, so the old U6bx open reads mode 0). A RW open
  claims a per-descriptor **writable staging buffer** from a small fixed pool (`NWSTAGE`, one page each,
  per-slot single-writer, `static mut` mirroring `HELLO_BYTES`), SEEDED from the file's staged content and
  tracked by a `+1`-biased `FILE_WSTAGE` sidecar; reserve/unwind is claim-last (slot→descriptor→handle) on
  every failure path. (3) **Routed writes** — `sys_write` is KIND-DISPATCHED at its single
  `handle_resolve(row, fd, CAP_WRITE)` CHECK: a `Console` streams to serial (byte-identical), a `File` with
  `CAP_WRITE` overwrites its writable buffer IN PLACE at the descriptor offset via `sys_write_file` — a pure
  **memcpy** (IF-masked-handler-safe), clamped to `min(len, size-offset)` (never grows), whole source
  validated up front (`-EFAULT` with no offset move), offset advanced by the count written. (4) **Read-back
  through the SAME cap** — `sys_read` serves a RW descriptor from its writable buffer, so a seek-back-and-read
  WITNESSES the write. The CHECK inherits U7x/U8x for free (the derivation/revocation walk lives inside
  `handle_resolve`, so a revoked write-cap is `-EACCES` at the write with no new code). **Three denials:** a
  File opened RO written to (rights arm), a non-File `Socket` carrying `CAP_WRITE` (kind arm), and a
  U8x-revoked File-write cap all `-EACCES`.
- **Honest scope — M1: IN-MEMORY ONLY, NO DISK WRITE-BACK.** `SCRATCH.BIN` is a const in-memory seed (1 KiB
  of `0xEE`, always present regardless of disk), NOT read from FAT; the read-back is the write witness — there
  is no on-disk "sector changed" check. **M2** (deferred, the completing twin) adds: capturing the FAT cluster
  chain at stage time + a per-descriptor dirty flag + a BSP-side write-back flush pump (reusing the existing
  shell-proven `fat::write_at`→`block::write_block` path from the polled main loop, IF=1) + planting
  `SCRATCH.BIN` on the x86 FAT image. M1 landed as a complete, honest capability milestone rather than
  half-landing the flush. Also deferred: file growth/create/delete/truncate; directory mutation; IF-safe
  interrupt-driven x86 storage (retires the staged-buffer divergence for both read and write); UnaFS
  `grants:*` on `SYS_OPEN` (K2/K3).
- **Tested — QEMU:** `UNAOS_FATIMG=sf ./arroyo test-fat sf 300` → after the U8x PASS: the U9x setup line and
  `:: U9x: real File writes — open-RW+seek+write+readback OK, RO-write/wrong-kind/revoked-cap all -EACCES
  (staged in-place, no disk write-back this milestone) -> PASS ::`. The `u9x-write` fixture (single slot,
  register-only apart from the read-back dest, witness `0x1F`) opens `SCRATCH.BIN` RW, seeks to a
  partial-sector offset (520), overwrites a 16-byte sentinel IN PLACE, seeks back and reads it through the
  SAME cap, and proves the RO-write + wrong-kind denials; `u9x_check_revoked_write` stages the U8x-revoked-
  File-write denial over scratch row 5 and demands the handle/file/writable-pool/derivation ledgers all clear.
  **Every prior U1a→U8x verdict byte-identical** (sorted scratch-worktree baseline diff vs `ca1b765` — pure
  append of the U9x lines). Default no-FAT `./arroyo test` stays MISSION SUCCESS AND U9x **runs and PASSes**
  there too — its in-memory scratch needs no FAT volume, which DIVERGES from the HELLO.BIN-dependent demos
  (U2/U4x/U6x/U6bx), all of which skip cleanly without a FAT. `./arroyo check` both arches, **zero aarch64
  files touched** (`arch/x86_64/syscall.rs` only — the write stack `fat.rs`/`block.rs`/`drivers/xhci` untouched).
- **Metal:** CANNOT be metal-confirmed — the standing x86 xHCI enumeration blocker (`task_47291f90`) means the
  rMBP enumerates no mass-storage device, so `block::info()` is None and U9x's storage-gated demo SKIPS on
  metal exactly as U8x/U7x do. QEMU-green is the ceiling until that blocker clears; the gate is NOT relaxed.

### U9x M2 — real disk write-back: FAT cluster capture, a per-descriptor dirty flag, a flush queue past teardown, and the launcher's IF=1 flush + raw-sector re-read (x86) 🔬 `hw-rmbp`
- **What:** completes the pi4 U9 twin — persists M1's staged in-memory write to the FAT on disk, the honest way
  around the IF-masked handler. M1 proved the whole capability surface with an in-memory `wstage` buffer (a
  read-back witnessed the write); M2 FLUSHES that dirty buffer to the FAT via the shell-proven
  `fat::write_at`→`block::write_block` path, at IF=1, OUTSIDE the SYSCALL handler, and the launcher raw-re-reads
  the sector to prove it landed. **THE crux (why M1 stopped here):** the dirty `wstage` buffer is freed at the
  fixture's teardown (`clear_files_row`, IF=0), but the flush needs IF=1 disk I/O — a naive "flush at teardown"
  cannot work. **Resolution — a flush queue that survives teardown, drained by the demo launcher at IF=1.**
  Teardown COPIES each dirty descriptor's bytes + its `(cluster, size, [lo,hi))` into a small static flush queue
  (the copy is self-contained — a stranded entry can never dangle at a freed buffer), then frees the `wstage`
  slot exactly as M1 (so the reviewed teardown-clear proof, `wstage_all_free()`, is undisturbed). The launcher —
  on the demo AP at IF=1, where the xHCI BOT pump self-services its own event ring (drains DMA'd ring memory +
  acks the interrupter via raw MMIO, so it drives from ANY CPU) — drains the queue via `fat::write_at` (in place,
  never grows), then raw-re-reads the sector.
- **The additions:** (1) a launcher **pre-flight** (IF=1, gated on `HELLO_STAGED`, the BSP's FAT-present signal)
  mounts + `find_in_root`s SCRATCH.BIN, validates it (regular file, non-zero chain head, size == the staged/wstage
  length), captures the pre-image bytes at the write offset, and publishes the chain head to `SCRATCH_CLUSTER`
  (Release) BEFORE the fixture opens — the x86 stand-in for pi4 capturing `FILE_CLUSTER` at open (x86 cannot walk
  the FAT in the IF-masked handler). (2) `sys_open` records `FILE_CLUSTER` from it; a per-descriptor `FILE_DIRTY`
  + dirty range `[lo,hi)` set by `sys_write_file` (fresh on the first write, so the flushed span is EXACTLY the
  touched sectors, never from offset 0). (3) The **revoke ordering** — a whole-task TEARDOWN (`clear_files_row`)
  ENQUEUES dirty; a REVOKE / open-unwind (`files_free`) DISCARDS dirty (revoke repudiates the write, so a revoked
  cap never flushes stale bytes). (4) `SCRATCH.BIN` (1 KiB of `0xEE`) planted on the x86 FAT image
  (`make-fat-img.sh`), mirroring the pi4 plant; its bytes equal M1's const seed, so the in-memory core is
  byte-identical whether or not a FAT is present. **Folded the two M1 review notes:** `sys_write_file`'s offset
  advance is now a tx-exact `compare_exchange` claim (CAS-symmetric with `sys_read`), and a writable (`CAP_WRITE`)
  open on `SHARED_ROW` is refused (a shared writable descriptor could race the unsynchronized staging memcpy).
- **DUAL MODE:** disk-backed (a FAT volume backs SCRATCH.BIN — `test-fat sf`) REQUIRES the on-disk write-back
  proof; in-memory (no FAT — plain `./arroyo test` attaches a non-FAT usb.img) runs the M1 core with the flush a
  no-op and does ZERO AP disk I/O (the pre-flight is `HELLO_STAGED`-gated, so a no-FAT run never issues an AP xHCI
  read). The revoke-discard proof is NON-VACUOUS: `u9x_check_revoked_write` first shows (positive control) that a
  dirty descriptor run through `clear_files_row` DOES enqueue a flush, THEN shows the revoke path leaves the queue
  empty — only the contrast proves discard. Verdict evolution: M1 `staged in-place, no disk write-back this
  milestone` → M2 `staged write FLUSHED to FAT (on-disk sector changed + size unchanged)`.
- **Tested — QEMU:** `UNAOS_FATIMG=sf ./arroyo test-fat sf 300` → the U9x line now proves the on-disk write-back:
  `:: U9x: real File writes — open-RW+seek+write+readback OK, RO-write/wrong-kind/revoked-cap all -EACCES, staged
  write FLUSHED to FAT (on-disk sector changed + size unchanged) -> PASS ::`. The launcher's raw re-read of the
  flushed sector (fresh mount, `read_at` chain-walk) finds the 16-byte pattern at offset 520, differing from the
  `0xEE` pre-image, with the directory size unchanged (1024) — the pi4 U9 proof, in QEMU. **All 16 prior
  U1a→U8x PASS/FAIL verdicts byte-identical** (sorted scratch-worktree baseline diff vs `ddefc40`; the ONLY
  verdict delta is the U9x line evolving M1→M2). (Two *descriptive* setup banners show best-effort-console-drop
  jitter — the baseline run itself dropped U7x's banner while this run kept it and dropped U6bx's — a pre-existing
  timing artifact of AP-side console contention, not a behavioral change: every demo ran and PASSed identically.)
  Default no-FAT `./arroyo test 25` stays MISSION SUCCESS AND U9x PASSes its in-memory core (`in-memory core; no
  FAT volume, flush is a no-op`), with zero AP disk I/O. `./arroyo check` both arches, **zero aarch64 files
  touched** (`arch/x86_64/syscall.rs` + `make-fat-img.sh` only — the write stack `fat.rs`/`block.rs`/`drivers/xhci`
  REUSED unchanged via its existing API). Adversarial code review (atomics/ordering, lifecycle/leaks, flush
  correctness, revoke-discard proof), 0 confirmed findings.
- **First concurrent AP-side xHCI BOT I/O in the tree:** the disk-backed flush is the first time a demo AP drives
  the hlt-pumped xHCI BOT engine while the BSP main loop also services xHCI (they serialize on the single
  `XHCI_CONTROLLER` lock). The pump is BOUNDED (a 2000-iter timeout → `Io`), so a failure would be a LOUD verdict
  FAIL, never a hang; `test-fat sf` is its empirical proof.
- **Metal:** unchanged from M1 — CANNOT be metal-confirmed (the xHCI enumeration blocker `task_47291f90` leaves
  `block::info()` None on the real rMBP, so the whole demo SKIPS there; QEMU-green is the ceiling, gate NOT
  relaxed). Unlike pi4 U9 (metal-confirmed on real EMMC2), x86 U9x waits on that blocker.
- **Deferred / tracked:** generation-tagged file-ids (a bare descriptor index lets a revoke+reopen alias a reused
  slot — real but not ring-3-reachable this arc; the x86 twin of pi4 U11, lands as **U11x**). File
  growth/create/delete (U10 twin); IF-safe interrupt-driven x86 storage (retires the staged-buffer divergence +
  the AP-flush detour); UnaFS `grants:*` on `SYS_OPEN`.
- **Commit:** `538a1bf` (`hw-rmbp`, Opus-executed).

### U11x — open-file lifecycle: SYS_CLOSE + generation-tagged file-ids (x86) 🔬 `hw-rmbp`
- **What:** the x86 twin of pi4 U11 M1 (`714daad`) — closes the U9x-tracked revoke+reopen aliasing gap. Before
  this, a per-task FILES descriptor was named to ring 3 by a bare `+1`-biased slot index; when a File revoke
  (`sys_cap_revoke`) freed a slot and a later `sys_open` first-fit-reused it, a lingering sibling handle carrying
  the old index would silently re-bind to the DIFFERENT file now living in that slot (the `FILE_USED` guard lapsed
  the moment the slot went live again). Fix = the standard slotmap/generation-index: a per-slot `FILE_GEN` counter
  bumped LAST on every free (`files_free` — the path SYS_CLOSE + the File-revoke drop + `sys_open`'s unwind route
  through — and `clear_files_row` at teardown); File handle words now pack `(gen << 32) | (idx + 1)`; and
  `file_desc_validate` is the SINGLE seam (range + `FILE_USED` + gen) every File consumer (`sys_read`/
  `sys_write_file`/`sys_seek`/`sys_close`, and the revoke File-drop) funnels through — a stale handle to a reused
  slot is `-EACCES` (a gen mismatch), never a rebind. Gen 0 packs to exactly `idx + 1`, byte-identical to the
  pre-U11x file-id, so every prior scaffold/kernel check that reads the value word is unaffected.
- **SYS_CLOSE (17):** frees the caller's descriptor slot (bumping its gen) + clears the handle word. Requires NO
  right (`handle_resolve(row, handle, 0)`); a non-File kind is `-EINVAL` (left intact), an unresolvable /
  already-closed / stale-slot handle is `-EBADF` (a double-close returns cleanly; a use-after-close is denied). New
  errno `EBADF = -9`. **x86 divergence from pi4:** `files_free` DISCARDS un-flushed dirty bytes (the staged
  write-back is a whole-task-teardown event, `clear_files_row`), so an explicit close of a dirty RW descriptor
  drops the write exactly as a revoke does — the demo closes RO handles; a future arc can make close enqueue the
  flush.
- **The x86 gap is revoke+reopen, not unlink:** x86 has no U10 (create/unlink), so — unlike pi4's fixture, which
  creates + unlinks — the aliasing vector here is a File revoke (which frees the descriptor) then a reopen reusing
  the slot. The gap is NOT ring-3-reachable at U9x (no way to hold a stale file-id across a free), so the ring-3
  fixture (`u11x-close`) proves SYS_CLOSE semantics (open+read → close → double-close `-EBADF` → use-after-close
  `-EACCES` → reopen+read; a 5-bit witness) over the immutable staged SCRATCH.BIN, while `u11x_check_gen_rebind`
  (kernel-side, scratch row 6) is the airtight no-rebind proof: claim a slot + mint its `(gen, idx)`; free it (gen
  bumps); re-claim the SAME slot (first-fit) at the bumped gen; prove the OLD file-id is rejected (gen mismatch)
  EVEN THOUGH the slot is live again, while a FRESH file-id resolves. Chained off `u9x_launcher` in program order;
  storage-gated (chain-inherited) but needs no disk itself (reads the static seed + pure descriptor bookkeeping),
  so it PASSes identically in FAT and non-FAT block-device modes.
- **Tested — QEMU:** `UNAOS_FATIMG=sf ./arroyo test-fat sf 300` → `:: U11x: open-file lifecycle —
  open+read/close/double-close(-EBADF)/use-after-close(-EACCES)/reopen+read OK, gen-tagged file-id rejects a stale
  sibling to a reused slot -> PASS ::`, with **all 17 prior U1a→U9x verdicts byte-identical**. Default no-FAT
  `./arroyo test 30` stays MISSION SUCCESS and U11x PASSes (block device present, no FAT needed). `./arroyo check`
  both arches green, **zero aarch64 files touched** (`arch/x86_64/syscall.rs` only). Every File value word backing
  a real descriptor is gen-tagged (`sys_open`, the U6bx no-cap plant, the U9x revoke-check plant); the two opaque
  scaffold ids (`0x100`/`0x200`) are untouched and resolve identically.
- **Metal:** storage-gated like the rest of the chain (chained off `u9x_launcher`), so the xHCI enumeration
  blocker (`task_47291f90`) keeps it off metal — QEMU-green is the ceiling. The gen-tag mechanism itself is
  always-on in the syscall path; only the demo is gated.
- **Commit:** `hw-rmbp`, Opus-executed.

### No-storage capability-chain visibility — inline console-cap demos (U5x/U7x/U8x) run on the metal path + `UNAOS_NOSTORAGE` knob 🔬 `hw-rmbp`
- **What:** the direct enabler for the FTDI metal bench. Every U-arc demo gated on `block::info().is_none()` and
  SKIPPED with no block device, so on the metal rMBP (where the SD reader never enumerates over xHCI —
  `task_47291f90` — leaving `block::info()` None) the whole capability chain was invisible. But U5x/U7x/U8x are
  **inline console-cap blobs needing NO storage** (no `SYS_OPEN`, no `staged_bytes`, no disk — they transfer/revoke
  `Console` caps and `sys_write` streams to serial); their gate was control-path DISCIPLINE (a clean no-storage
  boot log), not a functional dependency. With the FTDI cable making the no-storage metal path observable, the gate
  is **deliberately relaxed for those three demos only** (probe + launcher/run sites), surfacing the U5→U8 slice
  (capabilities → cross-process transfer → revocation trees) over the metal console without waiting for the xHCI
  storage fix.
- **Scope + safety:** surgical — the storage-GATED arcs (U2 load/U4x/U6x/U6bx/U9x/U11x, which genuinely need FAT /
  HELLO.BIN) KEEP their gates and still skip; NO protection touched (SMEP/NXE/W^X/page-perms are orthogonal — this
  is a demo-visibility gate); the relaxed demos are the already-reviewed U5x/U7x/U8x fixtures on an additional path,
  not new surface; relative demo order is preserved by the existing `*_LAUNCH_DONE` bounded waits, not the block
  gate. New builder knob **`UNAOS_NOSTORAGE=1`** omits the QEMU `usb-storage` device (block absent) — the QEMU
  analog of the metal no-storage path, and a preview of what the FTDI console replays.
- **Tested — QEMU:** `UNAOS_NOSTORAGE=1 ./arroyo test 90` → `note='no mass-storage device enumerated'` (block
  absent) with U5x/U7x/U8x **PASS**, every storage-gated arc absent (cleanly skipped), and U1a/U1b/U2-0/U3/U3.5
  still PASS. **No regression block-present** (the relaxation is a no-op there): `./arroyo test 30` stays MISSION
  SUCCESS with the applicable chain PASS; `UNAOS_FATIMG=sf ./arroyo test-fat sf 300` runs U1a→U11x with **0 FAIL**.
  `./arroyo check` both arches green, **zero aarch64 files touched** (`arch/x86_64/syscall.rs` + `builder/src/main.rs`).
- **Metal:** this is the metal ENABLER — the U5→U8 console-cap slice becomes metal-confirmable over the FTDI cable
  at the Peter-attended bench; the storage-gated arcs still wait on `task_47291f90`.
- **Commit:** `hw-rmbp`, Opus-executed.

### U10x — file GROWTH + CREATE + DELETE: cluster allocation, FAT-chain extension, directory mutation, all DEFERRED out of the IF-masked handler (x86) 🔬 `hw-rmbp`
- **What:** the x86 twin of pi4 U10 (M1 grow / M2 create / M3 delete), giving U9x's in-place File writer an
  ALLOCATOR — arch-symmetric with pi4 at the capability layer, diverging ONLY where the hardware forces it.
  **The FAT allocator is reused VERBATIM** from the shared `fs/fat.rs` (pi4 landed it: `write_grow`,
  `create_in_root`, `find_located`, `mark_dir_deleted`, `free_chain`, `delete_located`, `first_free_cluster`) —
  the port adds ZERO FS logic, only a read-only `cluster_size()` accessor (rmbp's shared-kernel-core lane) for the
  launcher's cluster-size-aware chain check. **THE load-bearing divergence (the same wall U9x M2 hit):** pi4 does
  grow/create/delete disk I/O straight in the SVC handler (EMMC2 PIO); x86 CANNOT — the SYSCALL handler runs
  IF-masked and the xHCI BOT pump `hlt()`s, so no disk I/O in-handler. So every U10 mutation is **DEFERRED** to the
  demo launcher's IF=1 drain (a SEPARATE U10 op-queue, leaving the metal-confirmed U9x in-place write-back path
  untouched). (1) **GROW** (`SYS_WRITE` past EOF on a growable descriptor) — the extend stages in the descriptor's
  one-page `wstage` buffer in-handler (`sys_write_grow`, size-before-offset, `FILE_SIZE == WSTAGE_LEN <= PAGE`
  invariant); teardown enqueues a `Grow` op the launcher drains via `fat::write_grow`. A per-descriptor
  `FILE_OPNAME` marks it growable (set at open ONLY for the staged GROW.BIN — SCRATCH.BIN stays in-place-only,
  U9x byte-for-byte) and the deferred op ALWAYS names the descriptor's OWN file (no confused-deputy: a RW
  SCRATCH.BIN holder can never mint a GROW.BIN op). (2) **CREATE** (`SYS_OPEN` `O_CREAT` of a name absent from the
  staged set) — a dynamic in-memory created-file model (`sys_open_dynamic`/`open_create_new`/`open_created_sibling`,
  the idempotent 2nd open + the sibling handle) backed by an empty `wstage`; teardown enqueues a `CreateGrow` op
  (`create_in_root`-if-absent + `write_grow`-from-empty). (3) **DELETE** (`SYS_UNLINK = 16`) — gated by the SAME
  single `CAP_WRITE` CHECK as write; a scaffold guard refuses an immutable STAGED file (HELLO.BIN is EL0 code —
  only a `FILE_CREATED` descriptor is unlinkable); marks the name gone for the row (`DYN_DELETED` → a plain
  re-open is `-ENOENT`), invalidates EVERY descriptor of the file (each `files_free` bumps the slot gen, so a
  stale sibling handle is `-EACCES` — the U11x gen-tag), and enqueues a `CreateGrowDelete` op the launcher replays
  (create + grow allocating a real cluster, a mid-op EXISTENCE witness, then `delete_located`).
- **Honest scope + faithful divergences (design + code adversarially reviewed):** create/grow/delete disk I/O all
  DEFER to the launcher — so the x86 DELETE is a launcher-side REPLAY of the fixture's create+grow+unlink (a weaker
  causal exercise than pi4's in-handler unlink of an independently-persisted file), and its bit3 (sibling read
  `-EACCES`) proves gen-invalidation, NOT a freed-chain aliasing fail-safe (physically impossible pre-drain on
  x86). The delete proof is made NON-VACUOUS by the drain's mid-op existence witness + `count == 1` + `drained`
  gating (a no-op drain cannot pass). Grow is one-page-bounded (invisible to the 16-byte demo). The FAT-safety
  invariants ride the shared `fat.rs` verbatim (all `num_fats` copies; zero-fill before chaining; dir `size` last;
  `0xE5` before free). Enqueue and drain are gated on the SAME "FAT present" signal so no op strands and no false
  in-memory PASS can mask a present volume (a code-review follow-up). Deferred, unchanged: subdirs/LFN/rename;
  IF-safe interrupt-driven x86 storage (retires the deferral); UnaFS `owner`/`grants:*` on open/create/unlink.
- **Tested — QEMU:** three fixtures (`u10x-grow` witness `0x1F`, `u10cx-create` `0xF`, `u10dx-delete` `0x1F`),
  each register-only apart from the read-back dest, chained `u9x → u10x → u10cx → u10dx → u11x` (every launcher
  chains the next on ALL paths, so a skip never strands U11x). `UNAOS_FATIMG=sf ./arroyo test-fat sf 300` (FAT32,
  512-B clusters) → on-disk grow (528B, 2-cluster chain, appended + original intact + all FAT copies consistent) /
  create (FRESH.BIN present, size 16, exactly one dir entry) / delete (DELME.BIN gone + chain freed all copies +
  cluster re-allocatable) all **PASS**; **20 PASS / 0 FAIL**, image cross-checked (GROW.BIN 528, FRESH.BIN
  count=1, DELME.BIN count=0). `test-fat p16` (FAT16 fixed-root, 2048-B clusters — the cluster-size-aware grow
  proof: 528B stays in one cluster, chain len 1) also **20 PASS / 0 FAIL**. Default no-FAT `./arroyo test 40`
  stays MISSION SUCCESS with all three U10 demos + U11x **PASS** in the in-memory core (deferred flush a no-op, no
  stranded op). `UNAOS_NOSTORAGE=1 ./arroyo test 90` skips the storage-gated arcs cleanly (console-cap slice
  U5x/U7x/U8x still PASS). `./arroyo check` both arches green, **zero aarch64 files touched** (`arch/x86_64/
  syscall.rs` + the read-only `fat.rs` accessor + the `make-fat-img.sh` GROW.BIN plant). Metal re-run self-heal
  (delete+recreate a prior boot's grown/created/deleted file via pub `fat.rs` primitives) keeps re-runs honest on
  a persistent card. **Capability chain U4→U10 now COMPLETE + arch-symmetric with pi4; unblocks U11x M2.**
- **Metal:** the storage-gated chain runs on real hardware only past `task_47291f90` — now closed by evidence at
  the 2026-07-08 bench (U1→U11x metal-confirmed off a FAT16 SD card), so a future attended bench can metal-confirm
  U10 by re-flashing; the self-heal makes it idempotent across reboots.
- **Commits:** `hw-rmbp` — M1 `6a54a76`, M2 `39ae5c5`, M3 `4471d34`, review fix `91c93b8`; Opus-executed.

### U11x M2 — cross-process open refcounts + unlink-defers-free: the deferred DELETE fires at the LAST close across processes (x86) 🔬 `hw-rmbp`
- **What:** the x86 twin of pi4 U11 M2/M2b (`b88d2ba`/`303e271`) — closes the U10-ledgered cross-process gap: a
  file open in ANOTHER process when unlinked no longer has its delete fire immediately; the deferred
  `CreateGrowDelete` op is enqueued **HELD** and released by the LAST descriptor release across all rows —
  explicit `SYS_CLOSE` or whole-task TEARDOWN (`free_user_space_by_cr3` → `clear_files_row`, the pi4 M2b
  exit-without-close orphan). The launcher's IF=1 drain plays the pi4 reaper (x86 needs no kernel task — the
  release is one atomic store, legal at IF=0).
- **Shape (x86-idiomatic, simpler than pi4 by construction):** the created-file identity space is the static
  `U10_NAMES` table, so the global refcount table is indexed DIRECTLY by name-id (`OPENF_REFS`/`OPENF_PENDING`/
  `OPENF_HELDSLOT`, pure atomics — no lock, no row allocation) instead of pi4's `SpinMutex` table keyed by the
  recyclable `(dir_lba, dir_off)`; and because an O_CREAT re-create of a delete-pending name is REFUSED with the
  new **`-EBUSY`** (until the delete drains — or, no-FAT, until the last-close release), the pi4 recycled-slot-key
  aliasing class (`b863304`) cannot exist here. The per-row `DYN_DELETED` overlay became GLOBAL (`DYN_DELETED_G`):
  unlink hides the name from EVERY row in-handler (plain re-open `-ENOENT` anywhere), cleared exactly when the
  delete completes. New capability: `sys_open` resolves a live created file in ANY private row
  (`created_desc_any_row`) — a cross-process sibling open snapshot-copies the source wstage (incref before
  `install_file_handle`, so its EAGAIN unwind through `files_free` pairs the decref exactly once — every
  release path funnels through the ONE `openf_decref` seam).
- **Proof (`u11m2_launcher`, chained after `u11x_launcher`; TWO phases over one EL0 fixture):** the launcher
  plays "process S" on a REAL allocated scratch slot (held across the phase so the fixture's allocator can never
  claim it): product-path `open_create_new` of DEFER.BIN + pattern write; the EL0 `u11m2-unlink` fixture (own
  slot) then proves witness `0x3F` — cross-row plain-RW open, read-back of the OTHER process's bytes, unlink → 0,
  invalidated-sibling read `-EACCES`, re-open `-ENOENT`, re-create `-EBUSY`. C1 (the DEFER, after fixture
  teardown): op still HELD + refcount 1 + pending + S's descriptor still reads the ORIGINAL bytes
  (cross-process read-after-unlink). Release phase 1 = the SYS_CLOSE core (`files_free`+`handle_clear`);
  phase 2 = the REAL teardown funnel (`free_user_space_by_cr3`). C2: released (drainable / refcount 0 / pending
  consumed). Drain: exactly ONE op, on-disk gone + chain freed in all FAT copies + cluster re-allocatable +
  name re-creatable again. In-memory (no-FAT) mode runs the identical witness + C1/C2 minus disk checks.
- **Review:** adversarial pre-code design review (1 reviewer, 5 confirmed must-fixes folded before coding:
  teardown `CreateGrow` suppressed for delete-pending names — else queue overflow + on-disk resurrection;
  deleted-flag check FIRST in the open path — else the any-row scan un-deletes a pending file; incref pinned
  before handle-install — else the EAGAIN unwind underflows the count and strands the name for the boot;
  scratch row from the real allocator — a hardcoded row a fixture could land on would leak the launcher's
  handle into ring 3; no-FAT release clears the deleted flag — else permanent `-EBUSY`). One code-level find at
  gate time (the wstage pool needed 3 concurrent buffers), fixed as `NWSTAGE = 3`.
- **Gate:** `./arroyo check` both arches (zero aarch64 files); `./arroyo test 40` MISSION SUCCESS, 17 PASS 0
  FAIL (U11m2 in-memory core PASS, all priors unchanged); `UNAOS_FATIMG=sf ./arroyo test-fat sf 300` **21 PASS
  0 FAIL** (U11m2 on-disk PASS appended, 20 priors); `UNAOS_FATIMG=p16 ./arroyo test-fat p16 300` 21 PASS 0
  FAIL (FAT16); `UNAOS_NOSTORAGE=1 ./arroyo test 90` clean skip (storage-gated, chain-inherited).
- **Honest divergences (ledgered in SECURITY.md):** the on-disk half is still the U10 launcher-side REPLAY;
  re-create-while-pending is `-EBUSY` (pi4 allows an immediate re-create as a new file); the cross-row sibling
  open is a SNAPSHOT copy (a concurrent writer in the source row could torn-copy, and a sibling that writes and
  exits would enqueue its own op against `NU10 == 1` — demo-sequenced, product fix = the pi4 lock + per-file
  backing); `SYS_CLOSE` still discards un-flushed dirty bytes (unchanged; now explicitly ledgered).
- **Metal:** storage-gated like the chain; the xHCI fix (`3bee9d6`) is metal-confirmed, so a future attended
  bench can metal-confirm U11m2 directly off the FAT card (self-healing pre-flight keeps re-runs honest).

---
## hw-jetson track — 2026-07-10 (code arc + same-day attended bench)

### JD2 — interactive shell on the Orin panel: keyboard → console → shell over the inherited scanout ✅ METAL-CONFIRMED (2026-07-10) `hw-jetson`
- **What:** the Orin's first interactive session — join JD1 (pixels) and JB10 (armed USB keyboard) with
  pure software routing, no new hardware touched. `tegra_early_stop` seeds `video::WRITER` with the JD1
  scanout; the JB2b `jb2-kbd` spawn becomes `jd2-console` (`main.rs::jd2_console_pump`, cooperative EL1
  task alongside CAPSTONE, `poll_events`-only/JC3 semantics): the boot log holds the panel until the
  **first keystroke**, then `fbcon::detach()` + a double-buffered `video::Screen` over the scanout and
  every key feeds the shared `handle_key` → `shell::dispatch_command` (keystrokes also echo on serial).
  Headless boots delegate to the JB2b `kbd_pump_body` unchanged. All `cfg(tegra)` — shared
  renderer/console/shell called, never edited.
- **Gate:** `UNAOS_TEGRA=1 ./arroyo check` both arches + `./arroyo test` + `test-arm` green (tegra off in
  QEMU → non-tegra byte-identical by construction). ⚠ `esp-jetson` `kernel.elf` = **378,728 B / 105
  `tegra:` strings** — the console/shell/FAT/font machinery is now linked in (+128 KB vs JD1), past the
  old ~355 KB heuristic; validate tegra media by the `tegra:`-string count, not size.
- **Metal (attended, 2026-07-10, same day):** keyboard ARMED direct-root port 6 slot 4 → pump live at
  EL1 → CAPSTONE → first key flipped the panel to the console (`console OWNS the panel`), `help` ⏎
  dispatched with every key echoed on serial and the output painted on the panel ("it works!"); the
  pump even survived a `gneiss` (vug) dispatch — keys kept flowing, no panic. The first interactive
  UnaOS session on the Orin. This session also flashed the SD card itself (the old EPERM was
  session-specific). Serial: `~/unaos-bench/jetson-serial-2026-07-10-090000.log`.
- **Detail:** [`arch_arm64.md` §JD2](dev/OS/01_BOOT_HAL/arch_arm64.md).

---
## hw-jetson track — 2026-07-08 (code arc — QEMU-green + adversarial-review-clean; USB-behaviour metal-pending)

### JD1 — first pixels: inherit the firmware's live scanout framebuffer ✅ METAL-CONFIRMED (Orin panel, 2026-07-08) `hw-jetson`
- **What:** get the boot log + CAPSTONE onto the Orin panel. JM7 found the panel dark because the UEFI
  GOP is `BltOnly` (no linear framebuffer) — but the firmware's DCE is still scanning out a DRAM carveout.
  **The finding (edk2-nvidia source):** the GOP is `BltOnly` *on purpose* — the default `SocDisplayHandoff`
  is SIMPLEFB, which hands the framebuffer off through the **device tree** instead: a `simple-framebuffer`
  node (geometry) → `memory-region` reserved-memory `reg` (the **physical** scanout base, with
  `iommu-addresses` declaring IOVA==PA). JD1 **inherits** that (the JB6→JB9 "inherit, don't re-init"
  pattern): a pure DTB walk — no display MMIO, no SMMU translation, no double-buffer hazard, no
  EL3-fatal-touch risk. `fdt_tegra::nvdisplay_simplefb` resolves it (+ `jd1_dump` diagnostic twin),
  `display_tegra::jd1_survey` decodes format/geometry + prints the `JD1 — scanout:` verdict,
  `mmu_tegra::map_fb_region` maps the carveout Normal-WB into **both** the EL2 `L1` and the EL1 twin (so
  it survives the JM6 drop), then `jd1_test_pattern` + `fbcon::init` bring the panel up. A read-only
  nvdisplay register sweep (`display_tegra::jd1_dc_survey`, `const JD1_DC_PROBE=false`) is the documented
  bench fallback for the case where the FDT we received carries no handoff node.
- **Register facts** (for the fallback), cross-checked against mainline `drm/tegra` `dc.h`/`hub.c` via a
  4-source research pass + adversarial verify (HIGH trust; the only bench-confirm number is the `0x10000`
  per-head stride): DC block `display@13800000`, per-window aperture `head+0x2800+0xC00·i`,
  `START_ADDR`/`_HI` at `+0x700`/`+0x734` (dword offsets ≪2), stride via `PLANAR_STORAGE`(×64), bit39 = a
  swizzle flag to mask.
- **Verified by construction + a 3-lens adversarial review** (MMU-correctness · off-tegra-neutrality ·
  DTB/scanout-safety, refuter-verified). QEMU never compiles `tegra`, so this is inert in every regression;
  the shared renderer (`video/framebuffer.rs`/`fbcon.rs`/`screen.rs`) is **unchanged** — JD1 only feeds it
  an address + geometry.
- **Tested:** `UNAOS_TEGRA=1 ./arroyo check` green both arches; `./arroyo test` (x86) + `./arroyo test-arm`
  (aarch64 virt) byte-green (all JD1 code `cfg(feature="tegra")` / inside `tegra_early_stop` → non-tegra
  byte-identical); `esp-jetson` `kernel.elf` **250,416 B / 101 `tegra:` strings** (up from JB10's 241,936 B
  / 90 — the JD1 survey/map/blit + linger code; RED LINE ~355 KB).
- **✅ METAL (2026-07-08, Peter at the Orin):** the firmware published the SIMPLEFB handoff into our FDT
  (`simple-fb /chosen/framebuffer 1920x1200 x8r8g8b8` → `framebuffer@0x279e00000` `0x960000` →
  `scanout base=0x279e00000 (Bgr) sane=true` → `panel LIVE`). On the panel: the colour-bar test pattern
  rendered **pixel-correct** (blue 2nd / red 5th → `Bgr` decode right; clean bars → stride right; framed +
  full-screen → base/geometry right), then fbcon painted the whole boot log + `CAPSTONE COMPLETE` across the
  EL2→EL1 drop. UnaOS's first correct frame on the Orin. `JD1_DC_PROBE` fallback never needed. A 3 s
  `JD1_TEST_PATTERN_HOLD_SECS` (`CNTPCT` busy-wait) keeps the pattern legible before the console takes over.
- **Detail:** [`arch_arm64.md` §JD1](dev/OS/01_BOOT_HAL/arch_arm64.md). Next: **JD2** — route the inherited
  USB keyboard to a live shell on the panel (first interactive UnaOS session on the Orin).

### JB10 — nested-hub descent + FS Evaluate-Context + root-kbd readiness + inherit-path housekeeping ✅ QEMU-green + review-clean / 🔬 metal-pending `hw-jetson`
- **What:** the four JB9-baton follow-ups. **(1) Nested-hub descent** (shared `xhci/mod.rs`, additive,
  hub-FSM is dead code under QEMU): `enumerate_downstream` detects a downstream hub (class `0x09`) and
  pushes it to `hubs_pending` so `service_hubs` descends another tier; `DeviceSlot` gains
  `route_string`/`route_depth`, `bring_up_hub` accumulates the Route String per tier (`| port <<
  (4·depth)`, 5-tier cap), and `address_downstream` programs the DW2 Transaction Translator for LS/FS
  children (HS/SS keep DW2=0 → working VIA-hub path byte-unchanged). **(2) FS EP0 Evaluate-Context**
  (`#[cfg(feature="tegra")]`, `JB10_FS_EVAL_CTX`, HYPOTHESIS): the JB9 baton's "needs a port reset" is
  refuted by the serial — the retry already resets + re-addresses at MPS0=64 and the FS device still
  goes silent, so the tear-down churn itself is the culprit. Adopts Linux `xhci_check_maxpacket`: read
  8 bytes, patch EP0 MPS0 in place via Evaluate Context (TRB 13, EP0 from the *output* context — the
  review caught the source-offset bug), read the full descriptor, no teardown; deferred to
  `service_enum` (`fs-mps-learn` stage), fallback to babble→recover. **(3) Root-keyboard demo:** no
  code — the path already arms a root HID end to end; item 2 helps a FS root keyboard. **(4)
  Housekeeping:** `JB9_PROBE` default-off (diagnostic suite only — recipe gates on `JB9G_NO_HCRST` /
  `JB5_PROBE`, untouched), two compile-time asserts making the FW-destroying levers un-co-enable-able
  with the inherit recipe, JB4 block wrapped in `!jb9h_skip`; forensic kit KEPT (flip back at a bench).
- **Verified by construction + a 5-lens adversarial review (1 CONFIRMED bug fixed pre-commit: the
  Evaluate-Context EP0 source offset).** QEMU cannot exercise items 1–3 (no `usb-hub`, lenient MPS) —
  they land as levers for the next attended bench; item 4 needs no bench.
- **Tested:** `UNAOS_TEGRA=1 ./arroyo check` green both arches; `./arroyo test` (x86) + `./arroyo
  test-arm` (aarch64 virt) byte-green (root storage/kbd/mouse enumerate — items 1–2 are dead-under-QEMU
  or `cfg(tegra)`, non-tegra byte-identical); `esp-jetson` `kernel.elf` **241,936 B / 90 `tegra:`
  strings** (the JB9_PROBE-off shrink from ~257 KB / ~100+ is intended, NOT a virt clobber; JB10 code
  present: `HUB-BEHIND-HUB`, `tegra fs-mps`).
- **Detail:** [`arch_arm64.md` §JB10](dev/OS/01_BOOT_HAL/arch_arm64.md). Next (attended bench): flash +
  watch nested descent (`storage_slot` past 0), the FS `fs-mps-learn` lever, and a direct-root
  `keyboard ARMED`; then a scoped arc to retire the dead JB3/JB4/JB5 chain code.

---
## hw-jetson track — 2026-07-08 (attended bench, Peter at the Orin)

### JB9 (bench outcome) — ⭐ USB WORKS ON ORIN: inherit + no-HCRST + 64-byte contexts ✅ metal-attended `hw-jetson`
- **The six-arc XUSB mystery is closed, verdict "other":** the fabric was never broken — the JB9f
  inherit-run probe (bare RS=1 on UEFI's halted state, no reset) posted a PSC event into UEFI's own
  >4 GiB event ring in 200 ms. Three real bugs, fixed live at the bench: (1) HCRST kills the
  inherited Falcon's service loop → `JB9G_NO_HCRST` halt-only takeover; (2) the JB3 fabric chain
  mutated a working config → `JB9H_SKIP_CHAIN` (the MC-override-reads-0x0 "torn-down link" that
  founded arc B is what the WORKING config looks like); (3) the Tegra xHC has `HCCPARAMS1.CSZ=1`
  (64-byte contexts) vs the driver's hard-coded 32-byte stride → ADDRESS_DEVICE code-17; fixed
  shared-driver-wide (`context::CTX_WORDS`, Peter-approved), + SuperSpeed+ PSI-5 MPS0 mapping +
  FS EP0 babble-learn retry + DISABLE_SLOT takeover eviction. **Result: both halves of a VIA hub
  fully enumerate on Orin silicon** (descriptors + hub bring-up over real DMA). Also proven: the
  ARU mailbox never answers NS even pre-EBS (JB9d loader bracket); a true PG cycle can't reload the
  FW (ROM has no bare IFR autoboot — ⚠ the cycle DESTROYS the only FW instance); AO IFRDMA regs are
  NS-locked. Next arc: nested-hub descent (devices sat behind hub layers, storage_slot still 0),
  port-7 FS retry reset, direct-port `keyboard ARMED` demo.
- **Detail:** [`arch_arm64.md` §JB9 bench verdict](dev/OS/01_BOOT_HAL/arch_arm64.md).

### JB9 (as-shipped) — FW-liveness without CPUCTL + DMA-path forensics 🔬→✅ see verdict above `hw-jetson`
- **What:** the two kernel-side probes the JB8 verdict demands. **A** (`jb9_fw_alive`, at
  raw-handoff / post-xhc-restart / post-enum-attempt): a CPUCTL-free liveness witness — fw-header
  identity via the ARU ioctl (+checksum/created-time), an ARU scratch heartbeat (two sweeps ~10 ms
  apart), and MSG_ENABLED with a patient 5-attempt/~100 ms retry ladder (vs JB3's one ~200 µs try)
  — one `FW-ALIVE`/`FW-SILENT` verdict line each. **B** (fired at t≈200 ms and t≈5 s inside the
  `jb2b_attach` pump window, i.e. WHILE enable-slot is pending): SMR/S2CR/context-bank dump for
  SID 0xe with an "is TTBR0 our identity table?" verdict (stale UEFI translation = the prime
  IOVA≠PA suspect), MC HOSTR/HOSTW + error log at that instant, the FW-side SID view (ARU
  STREAMID_FIELD + the AO IFR-autoboot trio, base DTB-resolved from padctl reg region 1), and a
  ±2 MiB near-target RAM scan for command-completion TRBs that landed at a wrong PA.
- **Tested:** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` green both arches; `esp-jetson`
  links (kernel.elf 269,480 B, 137 `tegra:` strings); `test-arm` green (`storage_slot=1`, zero
  panics — all JB9 code `tegra`-feature + `JB9_PROBE`-gated, QEMU byte-inert).
- **Detail:** [`arch_arm64.md` §JB9](dev/OS/01_BOOT_HAL/arch_arm64.md). Next: the attended bench
  fills the §JB9 verdict — which of {FW idled, SMMU stale context, SID mismatch, other}.

### JB8 — ⭐ METAL VERDICT: the Falcon was NEVER halted — CPUCTL is a CSB-locked register; the real failure is the DMA path ✅ metal-attended (USB reader) `hw-jetson`
- **The decisive read (boot 5, pre-EBS, driver live):** xHC running (`HCH=0 CNR=0`), USB enumeration in
  progress — and `CPUCTL`/`BOOTVEC` still read `0xffffffff` while the ARU fw-header ioctl answers. The
  Falcon is **alive and CSB-locked** (signed FW, raised priv level). Every JB3→JB7 "halted/reset-held"
  verdict read a locked register; JB7's "non-secure wall" dissolves — there was never a stopped core.
- **The real failure:** at kernel time port resets complete, 3 ports link-train to U0 (`CCS=1 PED=1`),
  but `enable-slot` times out — command/event-ring **DMA never touches DRAM, zero faults** (arc B's
  question reopened, now against a live engine). Plus: UEFI never programs FPCI CFG BARs (DT-fixed
  addressing only); auto-boots never connect XhciControllerDxe; generic edk2 XhciDxe's un-gated
  `XhcHaltHC` is the (benign) EBS actor; `DisconnectController` fails `INVALID_PARAMETER` (open).
- **Next:** kernel-side CPUCTL-free FW-liveness probe (mailbox retry + scratch heartbeat) + DMA-path
  forensics (SMMU S2CR/CB binding at write time, stale UEFI SMMU context, MC vs FW StreamID).
- **Detail:** [`arch_arm64.md` §JB8](dev/OS/01_BOOT_HAL/arch_arm64.md), bench verdict subsection.

### JB8 (as-shipped) — pre-EBS Falcon witness + reconnect lever; IFR-autoboot discovery 🔬→✅ see verdict above `hw-jetson`
- **What:** an edk2-nvidia source read (r36.4.0-updates) shows **UEFI never halts the Falcon core** (only
  BPMP PG/reset asserts at EBS — *both* teardown layers carry the JB6 ACPI skip) and **T234 starts it via
  IFR DMA autoboot** (three AO-aperture writes: `IFRDMA_CFG0/1` = fw-buffer PA, `STREAMID` = 0xE; buffer is
  `EfiRuntimeServicesData`, survives EBS) — plain NS MMIO, **no secure world** — so JB7's "NS-unstartable"
  verdict rests on a lever nobody has pulled. JB8 ships the discriminating loader-side probe:
  `jb8_falcon_witness` reads FPCI CFG + CSB `CPUCTL`/`BOOTVEC`/fw-header + `USBSTS.CNR` **pre-ExitBootServices**
  (is the Falcon already dead before handoff?), and a separate `jb8lever` risk media
  (`UNAOS_JB8_LEVER=1`) forces `Disconnect`+`ConnectController` on the `Usb2Hc` handles — a fresh
  `XhciControllerDxe.Start` (PG cycle + IFR load) right before handoff, with a post-reconnect re-read.
- **Tested:** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` green both arches; `esp-jetson` links with
  and without the lever feature (kernel.elf 254,536 B, 120 `tegra:` strings); `test-arm` green
  (`storage_slot=1`, zero panics — loader-side, tegra234-DTB-gated, QEMU inert). Metal: pending the
  attended bench (witness media first, lever media only if the witness reads dead).
- **Detail:** [`arch_arm64.md` §JB8](dev/OS/01_BOOT_HAL/arch_arm64.md). Next: bench; then the kernel-side
  IFR restart if the lever proves the path.

### JB7 — arc B refuted, arc A closed at the non-secure wall (Falcon core reset-held, unstartable from NS) ✅ metal-attended (native microSD) `hw-jetson`
- **What:** an offline read of the JB6 run-F serial **refutes arc B** (the "XUSB StreamID mismatch"): the
  MC override **sticks** at `0xe` (`rb=0x0000000e/0x0000000e`; the "reads `0x0`" was the pre-fix
  first-touch), the SMMU stream matches + identity-translates, and the fault census is **clean**
  (`sGFSR=0x0`, CB0 FSR unchanged, `MC INTSTATUS=0x0`, before *and* after attach). Zero faults ⇒ no XUSB
  DMA is even attempted — the empty event ring is **arc A's shadow** (a halted Falcon = a halted xHC
  command engine issues no completion), not a DMA-delivery failure. S2CR bypass (the baton's fix) was
  moreover already refuted on metal (boots 5/6). Added a read-only clock-census probe
  (`bpmp_tegra::jb7_clocks_query`, MRQ_CLK `IS_ENABLED`) to characterise the halt (`feature="tegra"` +
  `JB5_PROBE`-gated → QEMU byte-identical). A BAR2-vs-CFG CSB cross-read (`jb7_csb_cfg_read`) was added
  then **removed** — the metal boot proved the CFG aperture EL3-fatal (below).
- **Metal — ✅ attended (Orin, native microSD, 2026-07-08), arc A CLOSED at the non-secure wall:**
  (1) the alternate FPCI/CFG CSB aperture is **EL3-fatal** — first touch trapped to BL31 (`Unhandled
  Exception in EL3`, `esr_el3=0xbe000011`, EC 0x2F SError) and killed the boot; unrouted post-EBS, probe
  removed. (2) **Boot-medium bisect refuted** — first Falcon-witness boot off the **native microSD slot**
  (not a USB reader) still read `CPUCTL=0xffffffff`; the halt is universal, not the USB-boot path.
  (3) **Clock census** — core clock 269 **ON**, leaf 270/271 gated but not the lever (JB4 enabled them on
  metal; CPUCTL stayed dead) ⇒ core is **reset-held**, not clock-gated. Clean through **CAPSTONE 6/6**.
  With no `resets` on `usb@3610000`, MRQ_RESET can't reach the Falcon and the only BPMP reset (MRQ_PG
  cycle) is retired — so a **non-secure kernel cannot start the halted Falcon** (as JB5 was for revival).
  Next XUSB swing is bootloader/UEFI-side (suppress `XhciControllerDxe.Start` → inherit MB2's live Falcon)
  or a pivot off USB.
- **Tested — QEMU (the DONE gate):** `./arroyo check` + `UNAOS_TEGRA=1 ./arroyo check` both arches green
  (probes compile clean, zero new warnings); `UNAOS_TEGRA=1 ./arroyo esp-jetson` links (`kernel.elf`
  254,536 B tegra, 120 `tegra:` strings); virt `test-arm` green (`storage_slot=1`, byte-identical — all
  JB7 code gated off).
- **Detail:** [`arch_arm64.md` §JB7](dev/OS/01_BOOT_HAL/arch_arm64.md). Next: the attended arc-A bench.

### JB6 — "inherit, don't revive" the XUSB Falcon: a bootloader dummy-ACPI table makes UEFI skip its ExitBootServices teardown ✅ teardown-skip metal-proven / 🔬 enum → arcs A+B `hw-jetson`
- **What:** the bootloader installs a minimal, spec-correct dummy **ACPI 2.0 RSDP+XSDT** into the UEFI
  config table immediately before `ExitBootServices` (`install_tegra_acpi_shim`, gated on a `tegra234`
  DTB sniff so QEMU `esp-arm` stays byte-identical). NVIDIA's `XhciControllerDxe.OnExitBootServices`
  self-skips its XUSB teardown when an ACPI table is present, so UnaOS **inherits a powered,
  FPCI-configured XUSB block** instead of a torn-down dead one. The run-F kernel path is
  non-destructive (`JB5_RUN_E_REPLAY=false` retires the domain-power-cycle replay that would kill an
  inherited-live block; read-only witnesses + a `jb6_csb_sweep` diagnostic ship active behind
  `JB5_PROBE`). Preceded by **JB5**, which — 5 probe boots + a full edk2-nvidia source read —
  **refuted** non-secure Falcon revival (MB2 one-shot IFR self-boot; UEFI power-gate vote-refcounted).
- **Metal — ✅ teardown-skip PROVEN (Peter-attended, Orin, 2026-07-08):** A/B at raw handoff,
  no-shim → shim: XUSB power gates `0x0`→**`0x1`**, FPCI `busmaster=0`→**`1`**, BARs
  unprogrammed→**programmed**. Boot ran clean through **CAPSTONE 6/6**. **Not yet enumerating** — two
  root-caused next arcs: **(A)** the inherited Falcon *core* is halted (`CPUCTL=0xffffffff` read via the
  NVIDIA-correct T234 CSB path — the core is in reset / clock-gated behind a live ARU; needs a
  reset-deassert), **(B)** an XUSB MC StreamID mismatch (DMA tagged SID 0 vs the SMMU stream opened for
  SID 0xe → event-ring writes never reach DRAM).
- **Tested — QEMU (the DONE gate):** `./arroyo check` both arches green; `UNAOS_TEGRA=1 ./arroyo
  esp-jetson` links (`kernel.elf` 246 KB, healthy); virt `test-arm` green (`storage_slot=1`) with the
  shim correctly a **no-op** on the QEMU virt DTB (non-tegra path byte-identical).
- **Detail:** [`arch_arm64.md` §JB5+JB6](dev/OS/01_BOOT_HAL/arch_arm64.md). Next: arcs **A** (start the
  halted Falcon core) + **B** (MC StreamID) per the A+B baton.

---

## hw-pi4 track — 2026-07-07 (Opus round, post-Campaign 2)

### U9 — real File writes + seek: EMMC2 sector write, in-place `fat::write_at`, `SYS_SEEK`, File+`CAP_WRITE`-routed `sys_write` (aarch64) ✅ `hw-pi4`
- **What:** gives U6b's read-only `File`+`CAP_READ` its WRITE half — `CAP_WRITE` on a `File` now
  means something. Bottom-up: (1) **EMMC2 sector write** — `emmc2::write_block_512`, the exact mirror
  of the polled CMD17 read path with three deltas (`WRITE_SINGLE_BLOCK`/`cmd(24)` host→card so
  `CMD_DAT_DIR_READ` is omitted; the Buffer-**Write**-Ready bit `INT_WRITE_RDY = 1<<4`, distinct from
  the read path's bit 5; a FIFO that PUSHES 128 LE words, short buffers zero-padded to a full sector);
  same `send_command`, same bounded CNTPCT deadlines. (2) **Block seam** — `block::write_block` routes
  the SD backend to it (previously refused), inside the existing `#[cfg(aarch64, baremetal)]` arm so the
  x86 xHCI write path is byte-identical. (3) **`fat::write_at`** — the write twin of `read_at`: walks
  the file's existing chain, skips to `start`, read-modify-writes only the touched data sectors.
  BOUNDED BY CONSTRUCTION — clamped to `min(size, start+len)` so it **never grows** a file (write
  at/after EOF = 0-byte no-op), visits only in-chain clusters so it **never allocates or writes a FAT
  entry**, and **never touches a directory** (on-disk size + chain head unchanged). (4) **`SYS_SEEK = 15`**
  `(handle, offset)` — absolute seek; CHECK requires a `File` with ANY of `CAP_READ|CAP_WRITE`, past-size
  ⇒ `-EINVAL` (seeking TO `size` is legal), sets the U6b `FILE_OFFSET` (now settable). (5) **Routed
  writes** — `SYS_OPEN` gains a mode bit in `a2` (`0`=RO/`CAP_READ`, `1`=RW/`CAP_READ|CAP_WRITE`);
  `sys_write` is kind-dispatched at its single `handle_resolve(asid, fd, CAP_WRITE)` CHECK — `Console`
  streams to serial (byte-identical), `File`+`CAP_WRITE` overwrites in place at the descriptor offset via
  `sys_write_file`→`fat::write_at` (the write twin of `sys_read`'s clamp/validate/offset discipline).
  Because the U7/U8 derivation-revocation walk lives INSIDE `handle_resolve`, a revoked File-write cap is
  `-EACCES` at the write with **no new code**.
- **Tested — QEMU:** `./arroyo kernel8-test 30` → after the U8 PASS: the U9 setup line and
  `:: U9: real File writes — open-RW+seek+write+readback OK, RO-write/wrong-kind/revoked-cap all -EACCES,
  on-disk sector changed + size unchanged -> PASS ::`. The `el0-u9write` fixture (register-only, single
  slot, witness `0x1F`) opens a DEDICATED scratch file (`SCRATCH.BIN`, 1 KiB of `0xEE` planted by the
  launcher's FAT image — never `HELLO.BIN`) RW, seeks to a partial-sector offset, overwrites a 16-byte
  sentinel in place, seeks back and reads it through the SAME cap, and proves the RO-write + wrong-kind
  `-EACCES` denials; the launcher folds the kernel-side proofs — a fresh-mount raw re-read shows the
  sector CHANGED (to the sentinel, differing from the pre-image) while the directory size did NOT, and a
  scratch-ASID setup proves a U8-revoked File-write cap `-EACCES` (revoking a granted ancestor makes the
  derived `CAP_WRITE` resolve fail — exactly `sys_write`'s only gate). **Every prior verdict
  byte-identical** (sorted scratch-worktree baseline diff vs `e8db35c` — only the U9 lines + the
  binary-growth VBAR shift `0xad000`→`0xad800` differ), 14 prior PASS + U9 = 15, CAPSTONE 6/6, 0
  unexpected faults; `./arroyo test-arm 22` byte-identical (the driver is baremetal-gated); `./arroyo
  check` both arches; **zero x86 files**.
- **Honest scope:** IN-PLACE OVERWRITE ONLY. Deferred (each needs an allocation-policy pass): file
  growth / cluster allocation / create / delete / truncate; directory mutation; write-back caching
  (every write is a synchronous RMW to the card); UnaFS `owner`/`grants:*` on `SYS_OPEN` (the RW mode
  bit is the local precursor; the ACL check rides the kernel UnaFS mount, K2/K3). **Lane:** the seat
  widened the pi4 lane to the shared `fat.rs`/`block.rs` + the FAT image builder for this arc after the
  executor stopped and reported; `fat::write_at` is purely additive (x86 never calls it), the block seam
  + image plant are cfg-/pi-scoped.
- **Metal:** ✅ **METAL-CONFIRMED on the real Pi 4 (2026-07-07, same session).** Booted the self-contained
  UNAOS card; the EMMC2 driver identified the real card `@0xfe340000 15193 MiB CSD v2`, mounted its FAT32,
  and the `el0-u9write` fixture opened `SCRATCH.BIN` RW, overwrote it in place with a polled **CMD24**, read
  it back through the same cap, and the launcher's fresh-mount raw re-read confirmed the sector CHANGED while
  the size did NOT — `:: U9: … on-disk sector changed + size unchanged -> PASS ::`, with **15/15 PASS** (all
  prior + U9), **CAPSTONE 6/6** (all 4 cores online), and only the 3 expected M6b EL0 kills. **This is the
  first write the EMMC2 driver has ever issued to a card — the metal-risk write path (buffer-write-ready +
  FIFO push + transfer-complete/DAT0-programming-busy) is now proven on silicon**, the one thing QEMU's
  generic-sdhci could not vouch for.
- **Commit:** on `hw-pi4` (Opus-executed, post-Campaign 2; metal-confirmed same session).

## hw-rmbp track — 2026-07-07 (U8x — the round after Campaign 2, Opus-executed)

### U8x — revocation trees + generation-tagged inboxes: revoke chases derived capabilities; the sys_xfer TOCTOU closes (x86) 🔬 `hw-rmbp`
- **What:** the x86 twin of pi4 U8 on top of U7x, restoring FULL arch parity on the capability chain
  (U4→U8 now symmetric). Closes both escapes U7x's entry documents, slot-keyed throughout. (1) A bounded
  static **derivation ledger** (`MAX_DERIV = 16` nodes: state-exact CAS over a `0`/`RESERVING`/unique-id
  word, Release-publish/Acquire-read — the U7x discipline, no heap) records an edge child→parent at every
  derive: `sys_cap_grant` (local mint) and `SYS_XFER`/`SYS_RECV` (delivered transfer; the node rides the
  inbox slot + the sender-owned record). Revoke = mark ONE node; `handle_resolve` walks child→root
  (bounded, cycle-free by construction) and denies if ANY ancestor is revoked — **no revoke path ever
  writes another row** (U7x's stale-at-use pattern, generalized). `CAP_REVOKE` gets its real semantics:
  `SYS_CAP` REVOKE of a handle CARRYING it kills the whole derivation subtree (re-grants + re-transfers,
  however deep, exactly once, idempotently — a second revoke is `-ECHILD`); without it the drop stays
  local (U5x semantics, unchanged). XREVOKE marks the transfer's node too (id-guarded against reclaim
  ABA), so a re-transferred-onward cap dies with the root transfer — the U7x escape, closed. Node
  lifetime: frees on handle-drop when childless, else a **tombstone** until the subtree drains (cascading,
  CAS-arbitrated free). (2) **Generation-tagged inboxes:** a per-slot generation word (`SLOT_GEN` — the
  x86 twin of pi4's `ASID_GEN`) bumps at teardown before the sweep; deposits stamp it, RECV delivers only
  on an exact match, the sender post-check re-reads it — recipient-exit + slot-recycle + new-tenant-consume
  inside the deposit window can no longer deliver to the wrong tenant, from either side. x86 divergences:
  slot keying (`current_slot()`, `Proc.slot`); the SHARED_ROW stays refused as an endpoint; and the
  fixture conveys its witness via its name-routed `sys_exit` status (x86 has no SYS_REPORT).
- **Tested — QEMU:** `UNAOS_FATIMG=sf ./arroyo test-fat sf 260` → after the U7x PASS line: the U8x setup
  line, both fixture prints (`u8x: write via the grandchild cap`, `u8x: right-less revoke stays local`),
  and `:: U8x: revocation trees — parent revoke kills re-grant + re-transfer, generation-tagged inbox,
  ledger clean -> PASS ::` (14/14 PASS). The `u8x-tree` fixture (register-only, single slot) proves
  witness `0xF`: grant→re-grant chain works pre-revoke; parent revoke (with `CAP_REVOKE`) → 0 and its
  double revoke → exactly `-ECHILD`; BOTH descendants `-EACCES` at next use; a right-less revoke stays
  local (the derived copy still writes) + its own double-revoke errno. `u8_kernel_check` drives the REAL
  `sys_xfer_from`/`sys_recv_for` code paths over scratch rows 5/6/7 (all private, `< USER_SLOTS`, none the
  refused SHARED_ROW): an S→R1→R2 re-transfer chain, root XREVOKE ⇒ R1 **and** R2 stale + laundering
  refused (non-vacuous — the re-transferred cap carries `CAP_GRANT`); a deposit stamped before the
  teardown bump is never delivered to the recycled row (record freed; a late XREVOKE honestly `-ENOENT`);
  afterwards rows/inboxes/records/derivation ledger all provably clear. **Every prior U1a→U7x verdict
  byte-identical** (sorted scratch-worktree baseline diff vs `e8db35c` — pure append of the U8x lines; the
  U7x setup *banner* is a best-effort-console drop under peak 3-AP contention, not a regression — its PASS
  verdict, on the same branch-free path, lands byte-identical); default no-FAT `./arroyo test` stays
  MISSION SUCCESS (U8x needs no FAT file, so — like U7x — it runs and PASSes there too); `./arroyo check`
  both arches; **zero aarch64 files touched**.
- **Honest scope:** revocation + generations only. Carried from the aarch64 U8 review: `deriv_drop`
  retries the free-CAS past a racing revoke (a lost-CAS return would leak the node + tombstoned ancestors,
  exhausting the ledger under SMP); the laundering witness is non-vacuous. Deferred: the bandy Ring-3
  delegation wrapper, arbitrary recipients, `File` payload migration, real Socket syscalls, UnaFS
  `grants:*` on open, IF-safe x86 storage (retires the U6bx staged-source divergence).
  **Closed in the same merge window (deriv_drop SeqCst fence):** distinct from the carried
  revoke-vs-free race above, the `deriv_drop` tombstone/free *handshake* was a store-buffering (Dekker)
  race — a concurrent parent-vs-child drop of a cross-process chain could have both sides decline the
  free and leak a ledger node (fail-closed → eventual `-EAGAIN`; **no** UAF/escalation, the free is
  CAS-arbitrated). The identical Release/Acquire orderings shipped in the aarch64 U8 twin, so the fix
  promoted the four handshake ops (DROPPED store + KIDS load on the parent side; KIDS fetch_sub +
  DROPPED load on the child side) to `SeqCst` on **both** arches together — one total order forbids the
  double-stale outcome, the both-free case stays arbitrated by the `DERIV_ID` CAS. Surfaced by the U8x
  pre-merge concurrency lens.
- **Metal:** 🔬 metal-pending. The rMBP boot came up (kernel + USB enumeration fine) but **storage
  never enumerated** — an orthogonal x86 xHCI blocker (the SD-reader `058f:6362` enumerates
  unconfigured behind a hub; a hot-plug `ADDRESS_DEVICE` failed with a USB transaction error across 3
  resets), handed to the USB/xHCI track. So U8x's storage-gated demo **skipped rather than failing** —
  unexercised on metal, not a pass/fail. QEMU-green on `test` + `test-fat`; rides the next rMBP
  boundary once the xHCI enumeration path is fixed.
- **Commit:** on `hw-rmbp` (the round after Campaign 2, Opus-executed).

## hw-rmbp track — 2026-07-07 (Campaign 2, Fable-executed)

### U7x — cross-process capability transfer: inbox-mediated `SYS_XFER`/`SYS_RECV` + sender revoke, single-writer preserved (x86) 🔬 `hw-rmbp`
- **What:** the x86 twin of pi4 U7, restoring full arch parity on the capability chain — kernel-mediated
  delegation that preserves the single-writer-per-row invariant by construction. `SYS_XFER = 13` (dest =
  a `Child` handle in the SENDER's own table — owner-scoped; src must carry `CAP_GRANT`;
  `req & !src_rights != 0 -> -EACCES`) deposits the attenuated descriptor into the recipient's per-SLOT
  **inbox** (`NXFER = 4`; state word `0`/`RESERVING`/unique-tx; every claim/consume/retract/teardown a
  tx-exact CAS); `SYS_RECV = 14` installs it into the CALLER's own row. `SYS_CAP` XREVOKE (op 2) flips
  `XFER_REVOKED_BIT` inside the **sender-owned record's TX word** (`MAX_XFERS = 8`; tx-exact — the pi4
  review-fixed shape, inherited); the received cap goes stale at its next `handle_resolve` via the
  recipient-written `HANDLE_XFER_REC` sidecar; pending-revoked discarded at RECV; post-revoke
  grant/re-transfer laundering refused. pid→row rides a new `Proc.slot` (+1-biased — the `Proc.asid`
  substitution); post-check + retract closes the deposit-vs-exit race; teardown sweeps inbox + records
  and DISOWNS the dying sender's transfers. Payloads: `Console`/`Socket` only. x86 divergences: slot
  keying throughout, and the **SHARED_ROW refused as a transfer endpoint** (`-EACCES` both directions).
  **Folds both U6bx review notes:** the `FILE_OFFSET` advance is now a tx-exact CAS range-claim
  (well-defined for racing SHARED_ROW readers), and `SYS_CAP` REVOKE of a `File` handle frees its FILES
  descriptor (no more open→revoke `-EMFILE` exhaustion).
- **Tested — QEMU:** `UNAOS_FATIMG=sf ./arroyo test-fat sf 180` → after the U6bx PASS line: the U7x setup
  line, `u7x: child prints via the transferred cap` (a REAL console write authorized by a transferred
  capability), and `:: U7x: cross-process transfer — SYS_XFER attenuated, child received + used the cap,
  revoke enforced, single-writer intact -> PASS ::` (14/14 PASS). The launcher-orchestrated script
  proves: over-rights transfer refused; the **single-writer witness** (the child's handle row byte-clear
  while the t1 deposit sits in its inbox, the child provably parked pre-RECV); use-then-revoke ordering;
  the revoked cap `-EACCES`; teardown leaves rows/inboxes/records fully clear. Sequencing divergence
  from pi4: no x86 yield syscall, so each fixture runs on its OWN dedicated AP (3 APs required; fewer
  skips cleanly) with bounded-spin GO/RECV polls; witnesses ride name-routed exit statuses (no
  SYS_REPORT); the child's USE cue is a store to its own RW page. **Every prior U1a→U6bx verdict
  byte-identical** (sorted scratch-worktree baseline diff vs `225ae48` — pure append `33a34,35`);
  default no-FAT `./arroyo test` stays MISSION SUCCESS (U7x needs no FAT file, so — like U5x — it runs
  and PASSes there too); `./arroyo check` both arches; **zero aarch64 files touched**.
- **Honest scope:** single-LEVEL revoke — re-granted/re-transferred copies escape it (revocation TREES =
  the pi4-led U8: derivation records + real `CAP_REVOKE`); one documented TOCTOU residue at the sys_xfer
  post-check (generation-tagged inboxes ride the tree arc); a GRANT-minted duplicate File handle shares
  its descriptor — revoking either frees it, the survivor fails CLOSED.
- **Metal:** none expected (pure syscall logic; rides the next rMBP boundary with the battery).
- **Commit:** on `hw-rmbp` (Campaign 2, Fable-executed; adversarial review panel before merge).

## hw-pi4 track — 2026-07-07 (Campaign 2, Fable-executed)

### U8 — revocation trees + generation-tagged inboxes: revoke chases derived capabilities; the sys_xfer TOCTOU closes (aarch64) 🔬 `hw-pi4`
- **What:** closes U7's two documented escapes. (1) A bounded static **derivation ledger**
  (`MAX_DERIV = 16` nodes: state-exact CAS transitions over a `0`/`RESERVING`/unique-id word,
  Release-publish/Acquire-read — the U7 discipline, no heap) records an edge child→parent at every
  derive: `sys_cap_grant` (local mint) and `SYS_XFER`/`SYS_RECV` (delivered transfer; the node rides
  the inbox slot and the sender-owned record). Revoke = mark ONE node; `handle_resolve` walks
  child→root (bounded, cycle-free by construction) and denies if ANY ancestor is revoked — **no revoke
  path ever writes another ASID's row** (U7's stale-at-use pattern, generalized). `CAP_REVOKE` gets its
  real semantics: `SYS_CAP` REVOKE of a handle CARRYING it kills the whole derivation subtree
  (re-grants + re-transfers, however deep, exactly once, idempotently); without it the drop stays local
  (U5 semantics, unchanged). XREVOKE marks the transfer's node too (id-guarded against reclaim ABA), so
  a re-transferred-onward cap dies with the root transfer — the U7 escape, closed. Node lifetime:
  frees on handle-drop when childless, else a **tombstone** until the subtree drains (cascading,
  CAS-arbitrated free). (2) **Generation-tagged inboxes:** a per-ASID generation word bumps at teardown
  (before the sweep); deposits stamp it, RECV delivers only on an exact match, the sender post-check
  re-reads it — recipient-exit + ASID-recycle + new-tenant-consume inside the deposit window can no
  longer deliver to the wrong tenant, from either side.
- **Tested — QEMU:** `./arroyo kernel8-test 30` → after the U7 PASS: the U8 setup line, both fixture
  prints, and `:: U8: revocation trees — parent revoke kills re-grant + re-transfer, generation-tagged
  inbox, ledger clean -> PASS ::`. The `el0-u8tree` fixture (register-only, single slot) proves witness
  `0xF`: grant→re-grant chain works pre-revoke; parent revoke (with `CAP_REVOKE`) → 0 and its double
  revoke → exactly `-ECHILD`; BOTH descendants `-EACCES` at next use; a right-less revoke stays local
  (the derived copy still writes) + its own double-revoke errno. `u8_kernel_check` drives the REAL
  `sys_xfer_from`/`sys_recv_for` code paths over scratch ASIDs: an S→R1→R2 re-transfer chain, root
  XREVOKE ⇒ R1 **and** R2 stale + laundering refused; a deposit stamped before the teardown bump is
  never delivered to the recycled ASID (record freed; a late XREVOKE honestly `-ENOENT`); afterwards
  rows/inboxes/records/derivation ledger all provably clear. **Every prior verdict byte-identical**
  (sorted scratch-worktree baseline diff vs `225ae48` — only the U8 lines + the binary-growth VBAR/ELR
  shift differ), 19/19 prior PASS + U8 = 20, CAPSTONE 6/6, 0 unexpected faults; `./arroyo test-arm 22`
  byte-identical; `./arroyo check` both arches; **zero x86 files**.
- **Honest scope:** revocation + generations only. Deferred: the bandy Ring-3 delegation wrapper,
  arbitrary recipients, `File` payload migration, real Socket syscalls, UnaFS `grants:*` on open. The
  U7 RCsc-codegen footnote stands (the new code keeps the same ordering discipline).
- **Metal:** none expected (pure syscall logic; rides the next Pi boundary with the battery).
- **Commit:** on `hw-pi4` (Campaign 2, Fable-executed; U8 review: 3 lenses, 1 must-fix fixed in-arc + re-gated).

## hw-pi4 track — 2026-07-07 (Campaign 1 — the round-13 sweep, Fable-executed)

### U7 — cross-process capability transfer: inbox-mediated `SYS_XFER`/`SYS_RECV` + sender revoke, single-writer preserved (aarch64) 🔬 `hw-pi4`
- **What:** the first CROSS-process op on the object table — kernel-mediated delegation that preserves
  the single-writer-per-row invariant by construction. `SYS_XFER = 13` (dest = a `Child` handle in the
  SENDER's own table — owner-scoped; src must carry `CAP_GRANT`; `req & !src_rights != 0 -> -EACCES`)
  deposits the attenuated descriptor into the recipient's per-ASID **inbox** (`NXFER = 4`; state word
  `0`/`RESERVING`/unique-tx; every claim/consume/retract/teardown a tx-exact CAS); `SYS_RECV = 14`
  installs it into the CALLER's own row. `SYS_CAP` XREVOKE (op 2) flips the **sender-owned record**
  (`MAX_XFERS = 8`); the received cap goes stale at its next `handle_resolve` via a recipient-written
  sidecar (`HANDLE_XFER_REC`) — nobody ever writes a foreign row, not even to revoke. pid→ASID rides a
  new `Proc.asid` field; a **post-check + retract** closes the deposit-vs-exit race sender-side;
  teardown sweeps inbox + records (recycled ASIDs inherit nothing). Payloads: `Console`/`Socket` only
  (`File` = sender-local descriptor, refused; `Child` = reap delegation, refused).
- **Tested — QEMU:** `./arroyo kernel8-test 30` → after the U6b PASS line: the U7 setup line,
  `u7: child prints via the transferred cap` (a REAL console write authorized by a transferred
  capability), and `:: U7: cross-process transfer — SYS_XFER attenuated, child received + used the cap,
  revoke enforced, single-writer intact -> PASS ::`. The launcher-orchestrated script (GO words +
  cooperative SYS_YIELD polling — deterministic under QEMU) proves: over-rights transfer refused;
  the **single-writer witness** (the child's handle row byte-clear while the t1 deposit sits in its
  inbox, the child provably parked pre-RECV); use-then-revoke ordering; the revoked cap `-EACCES`;
  teardown leaves rows/inboxes/records fully clear. **Every prior M6/U4/U5/U6/U6b verdict
  byte-identical** (sorted scratch-worktree baseline diff vs `5ea4b48` — only the U7 lines + the
  binary-growth VBAR shift differ) + CAPSTONE 6/6 + 19/19 PASS + 0 faults; `./arroyo check` both
  arches; **zero x86 files**.
- **Honest scope:** single-LEVEL revoke — a re-granted or re-transferred copy escapes it (derived
  caps; the revocation-TREE arc adds derivation records); one documented TOCTOU residue at the
  sys_xfer post-check (exit + ASID-recycle + consume between two adjacent checks; generation-tagged
  inboxes ride the tree arc). Deferred: trees, the bandy Ring-3 wrapper, arbitrary recipients,
  `File` payload migration, real Socket syscalls.
- **Metal:** none expected (pure syscall logic; rides at the next Pi boundary with the battery).
- **Review:** five-lens adversarial panel + refuter verification (29 agents) — 2 distinct CONFIRMED
  must-fixes, **both fixed in-arc**: XREVOKE made tx-exact (the revoked flag moved into the record's TX
  word, bit 63 — the stale-revoke-on-reclaimed-record race and the born-revoked window are gone) and
  `u7_build` now scrubs the whole window (U6b's +0x3000 plant landed exactly on the GO word — the
  witness was racy). Notes closed: post-revoke laundering (`-EACCES` on delegation from a revoked
  received cap), sender-ASID-recycle revoke authority (teardown disowns records), `handle_row_is_clear`
  covers the transfer sidecar, planted-Proc truthfulness (EXITED on fixture exit/kill). Ledgered: the
  RCsc-codegen memory-model footnote. Re-gated green after the fixes (19/19 PASS, sorted-diff pure
  append).
- **Commit:** on `hw-pi4` (Campaign 1, Fable-executed; multi-lens adversarial review before merge).

## hw-rmbp track — 2026-07-07 (Campaign 1 — the round-13 sweep, Fable-executed)

### U6bx — real File handles: `SYS_OPEN`/`SYS_READ` through the object table, served from the BSP-staged source (x86) 🔬 `hw-rmbp`
- **What:** the x86 twin of pi4 U6b — makes U6x's `File` **scaffold** real, bringing the arches to
  File-handle parity: `SYS_OPEN = 11` mints a `File` handle carrying `CAP_READ` (first-free, skipping the
  reserved `CONSOLE_FD`; per-task FILES descriptor sidecars keyed `[row][idx]`, file-id = descriptor
  index + 1 so the value word stays clear of the `0`/`u64::MAX` sentinels); `SYS_READ = 12` is the
  enforcement point — `handle_resolve(row, handle, CAP_READ)` must yield a `File`, and a missing right /
  wrong kind / absent handle **all** return `-EACCES` (the `sys_write` Console+`CAP_WRITE` twin). Whole-
  destination validated up front as writable window memory (`-EFAULT`, no offset advance); offset-exact
  sequential reads, `0` = EOF; teardown-clear extends `clear_handle_row` to the FILES row (exit + kill).
- **The honest x86 divergence (the U4x pattern):** pi4 reads the disk INSIDE the SVC handler (EMMC2 is
  PIO). x86 cannot — the xHCI BOT read pump `hlt()`s and the SYSCALL handler is IF-masked — so `SYS_OPEN`
  serves the **BSP-staged set** (pre-read at IF=1 over the proven U2 FAT path; HELLO.BIN today, the same
  `stage_hello` buffer `sys_spawn` uses) and `SYS_READ` serves the staged bytes. The capability layer is
  byte-for-byte the pi4 twin; only the source of the bytes differs. Arbitrary-runtime-file open awaits an
  IF-safe / interrupt-driven x86 storage arc (which retires the divergence entirely).
- **Tested — QEMU:** `./arroyo test-fat sf 180` → after the U6x PASS line: the U6bx setup line and
  `:: U6bx: x86 real File handles — open+read via a File capability OK, no-CAP_READ -EACCES, wrong-kind
  -EACCES -> PASS ::` (13/13 PASS lines, 0 fault lines). The `u6bx-file` fixture opens HELLO.BIN, reads
  16 bytes through the cap and verifies them against the kernel-planted staged prefix, then proves both
  denial arms (a real-descriptor File with ZERO rights; a Socket WITH `CAP_READ`) — witness `0x1F`; the
  launcher proves the FILES-row teardown-clear kernel-side. **U1a/U1b/U2/U2.5/U3/U3.5/U4x/U5x/U6x all
  PASS byte-identical** (stash-free scratch-worktree baseline diff vs `e74dcb9` — pure append: no-FAT
  `32a33`, test-fat `37a38,39`); default no-FAT `./arroyo test` stays MISSION SUCCESS with U6bx skipping
  cleanly. `./arroyo check` both arches; **0 aarch64 files touched** (main.rs additions cfg-gated x86).
- **Honest scope:** the pre-staged set only; read-only, no seek/write/dirs. Deferred: x86 U7 (the pi4 U7
  twin), IF-safe storage (the divergence-retiring arc), real Socket routing, PCID, copy_from_user.
- **Metal:** pending (pure syscall logic; the staged source rides U2's metal-confirmed FAT path).
- **Review:** five-lens adversarial panel (cap-check / unwind / bounds / concurrency / regression) +
  refuter verification — **0 must-fix**, 2 deferrable notes ledgered in `SECURITY.md` (the non-atomic
  `FILE_OFFSET` advance under concurrent same-SHARED_ROW readers; REVOKE orphaning a File descriptor —
  both shapes shared with the merged pi4 twin; both fold into U7/U7x).
- **Commit:** on `hw-rmbp` (Campaign 1, Fable-executed; multi-lens adversarial review before merge).

## hw-rmbp track — 2026-07-06 (merged round 12, `9cc0326`)

### U6x — the general object table: `(kind, target, rights)`, first-free for all kinds, `CONSOLE_FD` collision closed (x86) 🔬 `hw-rmbp`
- **What:** the x86 twin of aarch64 U6a — generalizes U5x's fixed-shape handle into a general **object
  descriptor**, keyed by the address-space **slot**/`row` where aarch64 keys by ASID. The **kind** rides
  in a parallel sidecar `HANDLE_KIND[[AtomicU8; 8]; USER_SLOTS+1]` (keyed identically to
  `HANDLES`/`HANDLE_RIGHTS`), so the value word keeps U4x/U5x's sentinels **byte-identical** (`0`=Empty,
  `u64::MAX`=`RESERVING`) and nothing else is reserved — a `File(id)`/`Socket(id)` may carry any
  non-sentinel id with no high-bit masking (the STOP-tripwire sentinel collision is structurally
  impossible). `handle_resolve` dispatches `Child`/`Console`/`File`/`Socket` on the sidecar; `File`/`Socket`
  are resolvable **scaffolds** (no fs/net syscall routes through them yet).
- **`CONSOLE_FD` collision closed (the raison d'être + U5x's one design note):** `handle_install` (the
  first-free allocator) now **SKIPS** the reserved `CONSOLE_FD`, so a process that both prints (console cap
  at the fd=1/stdout index) AND spawns 2+ children (auto-allocated at `{0, 2, 3, ..}`) has **zero index
  collision for any interleaving** of installs — the console never clobbers, nor is clobbered by, an
  auto-allocated handle. The `fd=1` stdout ABI stays byte-identical for every existing blob. Every consumer
  is behaviour-preserved: attenuation unchanged, the mint copies the source's kind (never re-kinds) and
  publishes the value LAST; `handle_clear`/`clear_handle_row`/`handle_row_is_clear` also handle the kind.
- **Tested — QEMU:** `./arroyo test-fat part 30` (and `sf`) → after the U5x PASS line: the U6x setup line,
  `u6x: parent print (pre-spawn)` and `u6x: parent print (post-spawn; console survived 2 spawns)`, and
  `:: U6x: x86 general object table — printing spawner + 2 children, no index collision, File/Socket kinds
  resolve -> PASS ::`. `u6x-spawn` is the printing spawner U5x couldn't serve (prints → spawns 2 off the
  reserved index → prints again [console survived] → reaps both); `u6x_kernel_check` proves kernel-side that
  `File`/`Socket` kinds resolve with/without rights and that the exact U5x-breaking interleaving no longer
  collides. **U1a/U1b/U2/U2.5/U3/U3.5/U4x/U5x all PASS byte-identical** (proven by a stash-free scratch-
  worktree baseline diff — pure append of the U6x lines); the default no-FAT `./arroyo test` stays MISSION
  SUCCESS with U6x skipping cleanly (like U4x — its children need the staged program). `./arroyo check` both
  arches; **0 aarch64 files touched**.
- **Honest scope:** the OBJECT TABLE only. Deferred to U7 (the pi4 U7 twin): cross-process handle-transfer
  (`SYS_XFER`, breaks single-writer) + revocation trees (`CAP_REVOKE` still reserved) + the bandy Ring-3
  wrapper; real `File`/`Socket` fs/net routing through these kinds is a later arc. PCID and
  `copy_from_user`/`copy_to_user` stay separately deferred.
- **Metal:** pending (pure syscall logic; the child loads ride U2/U4x's metal-confirmed FAT path).
- **Commit:** on `hw-rmbp` (see landing report); unmerged (integrator records the merge).

### U5x — handles as capabilities: the CHECK + grant/attenuate/revoke + routed `sys_write` + teardown-clear (x86) 🔬 `hw-rmbp`
- **What:** the x86 twin of aarch64 U5 — turns U4x's handle STRUCTURE into a real **capability**,
  keyed by the address-space **slot** where aarch64 keys by ASID. A handle now carries **rights**
  (`CAP_READ|CAP_WRITE|CAP_EXEC|CAP_GRANT|CAP_REVOKE`, in a sidecar `HANDLE_RIGHTS` array keyed
  identically to `HANDLES`, so U4x's `0`/`RESERVING` value-word sentinels stay byte-unperturbed) and
  names a **target** beyond "child pid" (a `HANDLE_CONSOLE = u64::MAX-1` token — two kinds,
  `Child(pid)`/`Console`, no general object table = U6). **The CHECK** is a single
  `handle_resolve(row, idx, req_rights)` at the one lookup point every handle-consuming path uses:
  out-of-range/Empty/`RESERVING` ⇒ the caller's own errno (`sys_wait` → `-ECHILD`, U4x ownership
  preserved; `sys_write`/`SYS_CAP` → `-EACCES`), missing-a-right ⇒ `-EACCES`. **`SYS_CAP=10`** carries
  GRANT (mints an **attenuated** handle — `req & !src_rights != 0` ⇒ `-EACCES`, so a grant can never
  amplify; requires `CAP_GRANT` on the source) and REVOKE (ownership-based). **`sys_write` routes
  through the table** — `fd` is a handle that must resolve to `Console`+`CAP_WRITE`; no ambient stdout.
- **The x86 divergence (SLOT vs ASID):** the shared kernel window (U1a/U1b/U2 run with `user_cr3 == 0`,
  so `current_slot()` is `None`) has no private slot, so `HANDLES`/`HANDLE_RIGHTS` grow one extra row
  `SHARED_ROW` (index `USER_SLOTS` — the x86 twin of aarch64 ASID 0); `caller_row()` maps `None →
  SHARED_ROW`. The console cap is endowed there in `setup()` (covers U1a/U1b/U2) and per child in
  `sys_spawn`, so every prior print still lands. The fixture conveys its 4-bit witness as its `sys_exit`
  **status**, routed by task name (x86 needs no `SYS_REPORT`).
- **Teardown-clear** (folds U4x's one deferred note): `memory::free_user_space_by_cr3` wipes the slot's
  handle row (values + rights) **before** releasing the used-flag; both teardown paths (normal `exit` +
  the KillSwitch reap) funnel through it, so the clear rides both.
- **Tested — QEMU:** `./arroyo test-fat part 30` (and `sf`) → after the U4x PASS line: the U5x setup
  line, `u5x: cap write` twice (the write-cap + the minted cap reaching the console), and
  `:: U5x: x86 capabilities — write-cap OK, no-cap -EACCES, attenuated grant bounded, revoke enforced,
  teardown-clear clean -> PASS ::`. **U1a/U1b/U2/U2.5/U3/U3.5/U4x all PASS byte-identical** (routing
  `sys_write` drops no print — every printing process holds the endowed cap); the default no-FAT
  `./arroyo test` stays MISSION SUCCESS with U2/U4x skipping cleanly (U5x, being an inline fixture,
  still runs + PASSes). `./arroyo check` both arches; **0 aarch64 files touched**.
- **Honest scope:** register-only + cooperative fixture; deferred to U6 (the pi4 U6 twin) — bandy
  handle-transfer, a general object table (fs/net kinds, first-free `Console`), cross-process revocation
  trees (`CAP_REVOKE` defined but reserved); PCID and `copy_from_user`/`copy_to_user` stay separately
  deferred; FP/SIMD-across-context-switch stays ledgered (U4x left it register-only).
- **Metal:** pending (pure syscall logic; the child loads ride U2's metal-confirmed FAT path).
- **Commit:** on `hw-rmbp` (see landing report); unmerged (integrator records the merge).

## hw-rmbp track — 2026-07-05 (landed on `hw-rmbp`, awaiting integration)

### U4x — the process model + per-process handle table (x86) 🔬 `hw-rmbp`
- **What:** the x86 twin of aarch64 M7/U4 — a parent loads a child program into its OWN
  address space, runs it ring-3, and reaps it by an owner-scoped **handle**. **Part 0 (the
  enabler):** the `TSS.RSP0` + per-CPU `syscall_kernel_rsp` install moved from the ring-3
  trampoline into the scheduler **DISPATCH** path (beside U3.5's CR3-at-dispatch), so a task
  RESUMED after a block (which never re-enters the trampoline) gets ITS OWN kernel stack —
  the prerequisite for >1 concurrent user task per core, closing a use-after-free where a
  resumed task's syscall/fault would otherwise land on a sibling's (possibly freed) kernel
  stack. **Part A:** `SYS_SPAWN=8` returns a small handle index into the caller's per-process
  handle table (`HANDLES`, keyed by the caller's address-space **slot** — the x86 stand-in
  for aarch64's ASID, read from the live CR3 via `current_slot`); `SYS_WAIT=9` blocks on the
  child's done-semaphore, returns its status, and **consumes** the handle. `PROCS` (pid-keyed)
  and `HANDLES` (slot-keyed) are separate, static, const-init — the reviewed aarch64 U4 design
  adopted directly. **Part B:** a parent spawns two children and reaps both by distinct
  handles; an orphan's `sys_wait(0)` returns `-ECHILD` (proving the tables are per-process).
- **Load path — an honest x86 divergence:** aarch64 reads `HELLO.BIN` inside the SVC handler
  (its EMMC2 driver is PIO). x86 storage is USB-over-xHCI, whose BOT read pump `hlt()`s to
  await completion — and `hlt` with IF=0 hangs, while the SYSCALL handler is IF-masked. So the
  FAT read is pre-staged off FAT ONCE on the BSP main loop (IF=1, the proven U2 path) and
  `sys_spawn` only memcpys the staged bytes into a fresh slot. Same observable behavior.
- **Tested — QEMU:** `UNAOS_FATIMG=1 ./arroyo test 30` (and `test-fat part`/`sf`) →
  `:: U4x: x86 process model — parent reaped 2 children by handle, non-child sys_wait -ECHILD
  -> PASS ::`, with the two children each printing `hello from disk` (the 2nd/3rd in a full
  boot). **U1a/U1b/U2/U2.5/U3/U3.5 byte-identical** (proven by a pre-change baseline diff — the
  RSP0-at-dispatch move does not disturb the cooperative single-user-task path); U4x skips
  cleanly with no FAT volume (default / `UNAOS_USBSERIAL=1`). `./arroyo check` both arches; **0
  aarch64 files touched**. Two independent adversarial reviews (an 8-lens sweep + a 3-lens deep
  pass on the RSP0-at-dispatch use-after-free, the spawn/wait concurrency+leak+hang surface,
  and the blob's cross-syscall register ABI) each returned **0 findings**.
- **Honest scope:** register-only + cooperative fixtures (IF clear); `MAX_PROCS=4` + the static
  8-slot pool (STOP tripwires); no PCID; no `copy_from_user`/program-by-name; handle rows not
  cleared on slot teardown (harmless today — reapers consume handles; the capability CHECK +
  grant/attenuate/revoke + teardown-clear is U5). **FP/SIMD across a ring-3 context switch stays
  unsaved** (now reachable via Part 0's multi-task-per-core, not just U3.5 preemption) — the
  fixtures are register-only, so the gap is ledgered in SECURITY.md, not closed this arc.
- **Metal:** pending (rides the next reflash / FTDI cable day) — fully QEMU-verifiable (the reap
  wake is a scheduler post; child loads ride U2's metal-confirmed FAT path).
- **Commit:** on `hw-rmbp` (see landing report); unmerged (integrator records the merge).

## hw-jetson track — 2026-07-05 (landed on `hw-jetson`, awaiting integration)

### JM6 — drop the Orin boot core EL2 → EL1 + run the scheduler/CAPSTONE at EL1 🔬 QEMU-green / ⛔ metal FAILED `hw-jetson`
- **What:** repeats the JC3 drop on the **Orin** (Tegra234, Cortex-A78AE) boot core — it drops
  **EL2 → EL1** and runs the full six-primitive M4 CAPSTONE cooperatively at EL1, the first time
  the scheduler runs on Orin silicon. A new `arch/aarch64/boot_tegra.rs` (the tegra analogue of
  `boot_virt`) arms the EL1&0 regime at **`mmu_tegra`'s already-built identity `L1`** (`MmuInfo::
  ttbr0`) with `SCTLR_EL1.M=1` *while still at EL2* — dormant until the `eret`, so EL1 never runs
  an instruction with its MMU off — then a naked-asm drop (mask DAIF, seed VMPIDR/VPIDR, FP-enable,
  `HCR_EL2.RW`, **disable CNTP**, `SPSR_EL2 = 0x3c5`, `eret` to `x30`). `main.rs::tegra_early_stop`
  gains the JM6 terminus after JM4 (`fbcon::detach` → `boot_tegra::drop_to_el1(mmu.ttbr0)` →
  `percpu::init(0)` → `exceptions::install()` → `sched::run_capstone_boot_core(0)`, never returns).
  Single-core by design: JM5's Orin SMP (`CPU_ON`) is **parked** (metal-blocked on an external
  Tegra BL31/MCE RAS fault) and deliberately not invoked here, so JM6 sidesteps that wall.
- **Tested — QEMU (the DONE gate; Orin is not emulated):** `./arroyo check` both arches +
  `UNAOS_TEGRA=1 ./arroyo check` both legs, no new warnings. Non-regression: virt
  (`UNAOS_GICV3=1 test-arm 45`) SMP 3/3 + JC3 drop + `VBAR_EL1 = 0x7c38c000` + CAPSTONE 6/6 —
  **byte-identical** to JC3 (same VBAR address ⇒ virt binary layout unshifted); Pi (`kernel8-test`)
  **sorted-diff 0**; x86 (`test`) MISSION SUCCESS; `esp-jetson` media links. All JM6 code is
  `tegra`-gated, so every non-tegra build's cfg set (and output) is unchanged.
- **Metal — ⛔ FAILED (Peter-attended, Orin, 2026-07-05, 5 boots):** the boot core **dark-hangs at the
  EL2→EL1 drop.** JM3/JM4 + the heap init all run on silicon (every line through `:: tegra: JM6 —
  dropping … ::` prints), then dark. Localized: the `eret` reaches EL1 (no `VBAR_EL2` illegal-return
  fault), but the **first EL1 instruction fetch aborts** — a `VBAR_EL1` fault vector *and* a raw-UARTC
  sentinel stub armed before the eret both stayed dark, so `.text` is unexecutable at EL1 the instant
  the drop lands. Monitor-independent; `SCTLR_EL1`-independent (the `mmu_tegra` RMW pattern didn't help).
  Needs a dedicated investigation (see `arch_arm64.md` §3 JM6 result for the plan), NOT blind reboots.
  Captures `target/serial-orin-jm6-FAIL{,2,3,4}-*.log`.
- **Honest scope:** the reused EL2-built `L1` is correct for a kernel-only (no-EL0) core but not
  EL1-precise (RAM reads EL0-accessible via AP[1]=1; the device window is nominally EL1-executable
  though no code branches there) — an EL1-precise map is worth building only once EL0 runs on Orin.
  EL0-on-Orin and EL1 timer preemption (needs EL1-non-banking vectors) are follow-on arcs.
- **Commit:** this arc on `hw-jetson` (merge pending the integrator, who records the merge hash).

---

## hw-rmbp track — 2026-07-04 (landed on `hw-rmbp`, awaiting integration)

### U3.5 — preemptible ring 3 (x86) ✅ `hw-rmbp`
- **What:** completes the U3 process abstraction — a ring-3 task can now be dropped
  **preemptible** (`Task.preemptible` sets `RFLAGS.IF` in the `iretq` frame), so the
  local-APIC timer evicts it and other work shares its core. This closes the one-core
  DoS a program that never syscalls (`jmp $`) was. The x86 twin of aarch64 M6e. The
  timer ISR conditionally `swapgs`es on a CPL-3 entry so the scheduler sees the kernel
  per-CPU block, and the per-process **CR3 install moved from the trampoline into the
  scheduler DISPATCH path** so it covers a resumed-after-preemption task (which never
  re-enters the trampoline) as well as first entry. The full user register file is
  saved/restored across preempt/resume by the existing `x86-interrupt` + `switch_context`
  machinery. The cooperative fixtures (U1a/U1b/U2/U2.5/U3) stay `preemptible=false` — IF
  clear, never preempted — so they are byte-identical.
- **Tested — QEMU:** `./arroyo test 25` → `:: U3.5: ring-3 preemption — IRQs-at-ring3=160,
  co-task ran, spinner resumed -> PASS ::` (a preemptible spinner is preempted 160×, a
  kernel co-task on the same core runs to completion = the DoS fix, the spinner's
  private-CR3 counter climbs across preemptions = correct resume, and a watchdog reaps it
  via a scheduler `KillSwitch`). U1a/U1b/U2/U2.5/U3 byte-identical (only the U1a shared-blob
  size and 2 new U3.5 lines differ); coexists with the U2 disk loader (`UNAOS_FATIMG=1` →
  `hello from disk`), the U2.5 FTDI console (`UNAOS_USBSERIAL=1`), and the FAT regression
  (`test-fat part`/`sf`). `./arroyo check` both arches; 0 aarch64 files. Multi-lens
  adversarial review before commit.
- **Honest scope:** per-task opt-in (only the spinner is preemptible); FP/SIMD is NOT saved
  across preemption yet (no FXSAVE/FXRSTOR — the register-only spinner is safe); one user
  task per core (RSP0/`syscall_kernel_rsp` set at first entry only); no PCID.
- **Metal — ★ CONFIRMED (real 2012 rMBP, 2026-07-04, bootlog photo):** `:: SMEP on ::` (real
  supervisor-execute protection active while the preemptible spinner ran) then `:: U3.5: ring-3
  preemption — IRQs-at-ring3=156, co-task ran, spinner resumed -> PASS ::` — the real timer preempted
  the ring-3 spinner 156× and the swapgs-on-ring3-timer + CR3-at-dispatch + reap ran correctly on Ivy
  Bridge, every prior fixture PASS (U1a/U1b/U2-0a/U3 byte-consistent), 0 unexpected faults. The same
  boot also confirmed the U2.5 APIC ms-clock fix on metal: `initcnt=6236 [1 kHz calibrated]`,
  `ms-clock 999 Hz` (the old ~119 Hz reading is gone).
- **Commit:** on `hw-rmbp` (see landing report); unmerged (integrator records the merge).

---

## hw-pi4 track — 2026-07-04 (landed on `hw-pi4`, awaiting integration)

### U6b — real File handles: `SYS_OPEN`/`SYS_READ` routed through the object table via `File` + `CAP_READ` (aarch64) ✅ `hw-pi4`
- **What:** makes U6a's `File` **scaffold** real — the first resource syscall routed through a **non-Console**
  object, and the direct precursor to UnaFS grants (a program opening a disk file under a capability).
  **`SYS_OPEN = 11`**`(name_ptr, name_len)` → `copy_from_user`s the bounded 8.3 name, mounts the single
  read-only FAT volume, finds the top-level entry, records a per-task **open-file descriptor** and installs a
  `File` handle carrying `CAP_READ`; **`SYS_READ = 12`**`(handle, buf, len)` → the CHECK
  `handle_resolve(asid, handle, CAP_READ)` must yield a `File` (a missing right, a non-File kind, or no handle
  all give `-EACCES` — the twin of `sys_write`'s Console+`CAP_WRITE`), then it clamps to `min(len, size-offset)`,
  validates the destination (`user_range_ok(.., writable=true)` — a bad buffer is `-EFAULT` with no read and no
  offset move), reads through a new read-only **offset-aware** FAT reader (`fat::read_at`), `copy_to_user`s, and
  advances the descriptor's offset by the count delivered (`0` = EOF; sequential, no seek). The descriptor lives
  in a small **per-task FILES table** — parallel atomic arrays (`FILE_USED`/`FILE_CLUSTER`/`FILE_SIZE`/
  `FILE_OFFSET`, keyed `[asid][idx]`, `NFILE = 4`), the same lock-free sidecar shape as `HANDLE_RIGHTS`/
  `HANDLE_KIND`; the `File` handle's value word carries the **file-id = descriptor index + 1** (the `+1` bias
  keeps it clear of the value word's `0`/`u64::MAX` sentinels, structurally). **Teardown-clear** extends U5's
  discipline to files: `clear_handle_row` now also clears the FILES row, so a reused ASID starts with no stale
  file, no leaked offset, no aliasable descriptor. **Scope by design:** read-only, flat root, one FAT volume, no
  write/create/delete, no seek, no directory ops, no second mount — `SYS_OPEN` is the hook a later arc's UnaFS
  `owner`/`grants:*` enforcement rides. Lane: `arch/aarch64/syscall.rs` (the FILES table, `SYS_OPEN`/`SYS_READ`,
  the teardown-clear extension, the demo) + a `main.rs` launcher + `fs/fat.rs` (a read-only `read_at` +
  `first_cluster()` getter; `read_file` left **byte-identical** for its M6g/U4 caller); no scheduler primitive,
  no `boot.rs` change (the teardown-clear folds into `clear_handle_row`), no x86 file.
- **Tested — QEMU:** `./arroyo kernel8-test` → after the U6 PASS line: the U6b setup line and `:: U6b: real
  File handles — open+read via a File capability OK, no-CAP_READ -EACCES, wrong-kind -EACCES -> PASS ::`. The
  `el0-u6bfile` fixture opens `HELLO.BIN`, reads its first 16 bytes through the returned `File` capability and
  verifies they equal the kernel-planted on-disk prefix (`USER_BLOB[..16]` — `HELLO.BIN` on the media *is*
  `USER_BLOB`), then proves the CHECK denies both a **present File lacking `CAP_READ`** (the rights arm) and a
  **`Socket` carrying `CAP_READ`** (the kind arm, `SYS_READ` serves `File` only) with `-EACCES` — witness `0x1F`
  (all five bits). The launcher additionally proves the **file-row teardown-clear** kernel-side (the fixture
  exits holding two live descriptors — its own open + a pre-endowed no-cap File — so `files_row_is_clear`
  transitions false→true when its slot retires). Every M6b/M6d/M6e/M6f/M6g/U4/U5/U6 verdict line
  **byte-identical** (the shared FAT mount does not regress M6g/U4 — both still PASS their disk loads) and
  CAPSTONE 6/6, 0 unexpected faults. `check` both arches; x86 `test` MISSION SUCCESS; aarch64 virt v2 clean USB
  enumeration + GICv3 CAPSTONE 6/6 unchanged; zero x86 files.
- **Tested — metal:** ✅ **METAL-CONFIRMED on the real Pi 4 (2026-07-06)** — Peter booted (non-destructive
  `kernel8.img` swap on the mounted FAT; `HELLO.BIN` was already byte-identical to this build, so the
  bytes-match test is exact), I ran the Debug-Probe serial bridge. On silicon: `:: U6b: real File handles —
  open+read via a File capability OK, no-CAP_READ -EACCES, wrong-kind -EACCES -> PASS ::` — the fixture opened
  `HELLO.BIN` and read it through a `File` capability off the **real EMMC2/SD card** (the metal-only EMMC2-first
  leg `SD card @0xfe340000 identified — 31116288 blocks (15193 MiB, CSD v2)`, which QEMU cannot exercise), and
  both `-EACCES` denials held. `EL=1`/`CNTFRQ=54 MHz`, EL0 preempt live (`M6e IRQs-taken-at-EL0=23`,
  `M6f spsentinel=2`), full battery green (M6b `exited=1 killed=3`, M6d ×3, M6f ×3, M6g/U4/U5/U6 PASS), 0
  unexpected faults. (The scheduler CAPSTONE demo sat out this particular boot — only 3 of 4 cores came online,
  and it needs the full 4; that is a known metal SMP AP-bring-up variance in the scheduler track, orthogonal to
  U6b's pure-syscall logic.) Metal log `unaos/target/serial-pi.u6b-metal.log`.
- **Deferred:** file **writes**/create/delete, **seek**/`lseek`, **directory** ops (the natural extensions once
  read-only File handles are proven); real **`Socket`** handles (net syscalls — the fs twin of this arc); UnaFS
  `owner`/`grants:*` checked on `SYS_OPEN` (rides the kernel UnaFS mount, K2/K3); a second mount / media detect.
- **Commit:** this arc on `hw-pi4` (merge pending the integrator, who records the merge hash).

### U6a — the general object table: `(kind, target, rights)` descriptors, first-free for ALL kinds, the `CONSOLE_FD` collision closed (aarch64) ✅ `hw-pi4`
- **What:** generalizes U5's fixed-shape handle into a general **object descriptor**. A handle now
  names one of four **kinds** — `Child(pid)` (U4), `Console` (U5), and the **scaffolds** `File(id)` /
  `Socket(id)` (defined + resolvable via `handle_resolve`, but no fs/net syscall routes through them
  yet — they prove the table is genuinely general, not that fs/net exists). The **kind rides in a
  parallel sidecar** `HANDLE_KIND[[AtomicU8; 8]; USER_SLOTS+1]` (keyed identically to `HANDLES`/
  `HANDLE_RIGHTS`), so the value word keeps U4/U5's sentinels **byte-identical** — `0` = Empty (the
  lock-free allocator's free marker), `u64::MAX` = `RESERVING` — and nothing else is reserved. Picking
  the sidecar over the value word's high bits makes the sentinel-collision STOP tripwire *structurally
  impossible* (a `File`/`Socket` id may be any non-`0`/non-`u64::MAX` word, no masking) and mirrors the
  rights sidecar 1:1 (kind + rights published Release BEFORE the live value; single-writer-per-ASID the
  backstop). **The `CONSOLE_FD` collision is closed** (the arc's raison d'être + U5's one design note):
  U5 pinned the console cap at a fixed index via an unconditional store while `handle_install`'s
  first-free scan allocated from index 0, so a process that both PRINTED and SPAWNED 2+ children could
  auto-allocate a child onto index 1 and have it clobbered by the console install. U6 makes `CONSOLE_FD`
  a **reserved index the first-free allocator SKIPS**: the console lives there by the `fd=1`/stdout
  convention (keeping every prior blob byte-identical), children/objects fill `{0, 2, 3, ..}`, so a
  console cap and N child/object caps coexist with **zero index collision for any interleaving of
  installs**. Every consumer is behaviour-preserved: `handle_resolve` dispatches on the kind sidecar
  (`sys_wait`→`Child`, `sys_write`→`Console`+`CAP_WRITE`, `sys_cap` grant/revoke on any kind); the
  **attenuation invariant is unchanged** and the mint copies the source's kind (attenuate rights, never
  re-kind); `handle_clear`/`clear_handle_row`/`handle_row_is_clear` also handle the kind. Lane:
  `arch/aarch64/syscall.rs` (the descriptor, reserved-index alloc, resolve, kind scaffold, the demo) +
  a `main.rs` launcher; no `boot.rs` change (`clear_handle_row` already wipes the whole row), no
  scheduler primitive, no driver, no x86 file.
- **Tested — QEMU:** `./arroyo kernel8-test` → after the U5 PASS line: the U6 setup line, `u6: parent
  print (pre-spawn)`, `u6: parent print (post-spawn; console survived 2 spawns)`, the two children's
  `hello from EL0`, and `:: U6: general object table — printing spawner + 2 children, no index
  collision, File/Socket kinds resolve -> PASS ::`. The `el0-u6spawn` fixture is the printing spawner U5
  could not serve: it prints, spawns 2 children (distinct auto-allocated handles, both `!= CONSOLE_FD`),
  prints AGAIN (the console cap survived the spawns), and reaps both by handle — witness `0xF`. The
  launcher additionally proves kernel-side that the `File`/`Socket` kinds resolve to their kind with the
  required right (and `Denied`/`-EACCES`-equivalent without) and that the exact U5-breaking
  console-vs-two-children interleaving no longer collides (`u6_kernel_check`). Every
  M6b/M6d/M6e/M6f/M6g/U4/U5 verdict line **byte-identical** (sorted set-diff: the only delta is
  `VBAR_EL1` shifting one page — benign binary growth from the added code — plus the new U6 lines) and
  CAPSTONE 6/6, 0 unexpected faults. `check` both arches; x86 `test` MISSION SUCCESS; aarch64 virt v2
  clean USB enumeration + GICv3 CAPSTONE 6/6 unchanged; zero x86 files.
- **Tested — metal:** none this arc — U6 is fully QEMU-verifiable (descriptor/allocator/resolve logic;
  the demo's two children ride U4/M7's already-metal-confirmed EMMC2 load path).
- **Deferred:** **U7** — cross-process handle-transfer (`SYS_XFER`, a cross-ASID write discipline that
  breaks the single-writer invariant) + revocation trees (`CAP_REVOKE`, reserved) + the bandy Ring-3
  delegation wrapper; and a later arc — real `File`/`Socket` fs/net syscalls routing through these
  kinds, plus UnaFS `owner`/`grants:*` enforcement on `SYS_OPEN` (rides the kernel UnaFS mount).
- **Commit:** this arc on `hw-pi4` (merge pending the integrator, who records the merge hash).

### U5 — handles as capabilities: the enforcement CHECK + grant/attenuate/revoke + routed `sys_write` + teardown-clear (aarch64) ✅ `hw-pi4`
- **What:** turns U4's handle STRUCTURE into a real **capability**. A handle now carries
  **rights** — a bitmask `CAP_READ|CAP_WRITE|CAP_EXEC|CAP_GRANT|CAP_REVOKE` in a **sidecar**
  `HANDLE_RIGHTS[[AtomicU32; 8]; USER_SLOTS+1]` keyed identically to `HANDLES`, so U4's
  `0`/`RESERVING` value-word sentinel logic stays byte-unperturbed — and names a **target**
  beyond "child pid" (a well-known `HANDLE_CONSOLE = u64::MAX-1` token; two kinds only,
  `CHILD(pid)` and `CONSOLE`, not a general object table — that is U6). The **CHECK** is a
  single `handle_resolve(asid, idx, req_rights)` at the one lookup point every handle-consuming
  path goes through: out-of-range/Empty ⇒ the caller's own errno (`sys_wait` → `-ECHILD`, U4's
  structural ownership preserved; `sys_write`/`SYS_CAP` → `-EACCES`), missing-a-right ⇒
  `-EACCES`. **`SYS_CAP` (10)** adds grant/attenuate/revoke: GRANT mints a new handle to the
  same target carrying a rights mask that must be a **subset** of the granter's rights on the
  source — the **attenuation (monotonic-decrease) invariant**, `req & !src_rights != 0` ⇒
  `-EACCES` (a grant can never amplify), requiring `CAP_GRANT` on the source; REVOKE drops a
  handle the caller owns (subsequent use ⇒ `-EACCES`/`-ECHILD`). **`sys_write` routes through
  the table** — `fd` is a handle index that must resolve to a `CONSOLE` handle with
  `CAP_WRITE`; no ambient stdout. Every printing EL0 process is **endowed** a `CONSOLE`+
  `CAP_WRITE` cap at `CONSOLE_FD = 1` at spawn/launch (shared window ASID 0 for `el0-hello`;
  each M6f/M6g/U4-child slot), and the `copy_from_user`/all-or-nothing `-EFAULT` path is
  byte-identical (the M6f hostile fixture holds the cap, so its bad-pointer writes still
  `-EFAULT`, not `-EACCES`). **Teardown-clear** folds U4's one deferred note:
  `boot::teardown_user_slot` wipes the whole `HANDLES[asid]` row + rights **before** releasing
  the slot's used-flag (not after — a post-release clear could race a concurrent
  `alloc_user_slot` on another core reclaiming the ASID), so no capability outlives its ASID.
  Lane: `arch/aarch64/syscall.rs` + a `boot.rs` row-clear (in `teardown_user_slot`) + a
  `main.rs` launcher; no scheduler primitive, no driver, no x86 file.
- **Tested — QEMU:** `./arroyo kernel8-test 8` → after the U4 PASS line: the U5 setup line,
  `u5: cap write` **twice** (the write-cap write + the write through the minted attenuated
  cap), and `:: U5: capabilities — write-cap OK, no-cap -EACCES, attenuated grant bounded,
  revoke enforced, teardown-clear clean -> PASS ::`. The `el0-u5cap` fixture proves four
  EL0-observable behaviours against its own table (write-cap OK; a write-less cap → `-EACCES`;
  a grant exceeding the granter's rights rejected while a subset grant works and its handle
  writes; a revoked handle → `-EACCES`) via a witness bitmask (`0xF`), and the launcher proves
  the fifth kernel-side (the fixture's handle row is clear after its slot teardown). Every
  M6b/M6d/M6e/M6f/M6g/U4 line byte-identical (hex/pid-normalized set-diff: only the four new U5
  lines added; all four prior `hello from EL0` land, the M6f `4 hostile … EFAULT` PASS holds,
  U4 PASS holds, CAPSTONE 6/6) and 0 unexpected faults. `check` both arches; x86 `test` MISSION
  SUCCESS; aarch64 virt v2 MISSION SUCCESS + GICv3 JC3 SMP 3/3 + CAPSTONE 6/6 unchanged.
- **Tested — metal:** none this arc — U5 is fully QEMU-verifiable (the checks/grants/revokes
  are pure syscall logic; the reap wake is a scheduler post; the child disk-loads ride
  U4/M7's already-metal-confirmed EMMC2 path). A future reflash would re-exercise the endowed
  prints off the real card, but nothing in U5 is metal-*gated*.
- **Commit:** this arc on `hw-pi4` (merge pending the integrator, who records the merge hash).

### U4 — the process model + per-process handle table (aarch64) ✅ `hw-pi4`
- **What:** the ownership half of the process model — `sys_spawn` now returns a **handle**
  into the *caller's* per-process handle table (not a raw pid) and `sys_wait` takes that
  handle, so reaping is **structurally owner-scoped**: a task can only reap children whose
  handles are in ITS table (folding M7's review note — any task could `sys_wait` any pid —
  by construction). The table is a static, const-init `HANDLES[[AtomicU64; 8]; USER_SLOTS+1]`
  keyed by the caller's **ASID** (read from `TTBR0_EL1[63:48]` synchronously in the SVC
  handler); `PROCS` stays keyed by pid (exit-accounting control blocks) while `HANDLES` is
  keyed by ASID (the spawner's private namespace of child capabilities) — deliberately
  separate. No new syscall number, no new scheduler primitive, no driver, no boot change:
  the whole arc is `arch/aarch64/syscall.rs` + one `main.rs` launcher tweak. `sys_write`
  stays the `fd==1` path (routing a resource syscall through a handle is U5, when there is a
  capability *check* to add). This is the exact substrate U5 turns into capabilities (grant =
  transfer a handle, revoke = clear it; U5 adds the check at this same handle lookup).
- **Tested — QEMU:** `./arroyo kernel8-test 30` → in place of the M7 line: the U4 setup line,
  **four** `hello from EL0` (M6c inline #1, M6g loader #2, the two U4 children #3/#4), and
  `:: U4: process model — parent reaped 2 children by handle, non-child sys_wait -ECHILD
  (per-process handle tables) -> PASS ::`. The demo: a parent (`el0-u4parent`) `sys_spawn`s
  two children and reaps **both by handle**; an ownership negative (`el0-u4orphan`, its own
  slot/ASID) calls `sys_wait(0)` on an Empty handle and gets `-ECHILD`. Every
  M6b/M6c/M6d/M6e/M6f/M6g + CAPSTONE line byte-identical (hex/pid-normalized set-diff: only
  the M7 line → the U4 line, `hello` 3→4) and 0 unexpected faults. x86 (`test` +
  `UNAOS_FATIMG=1 test`) functionally byte-identical through the U-lines (seam is
  `baremetal`-only; sole diff is a QEMU timer/scheduling jitter on the async U2 exit at the
  25 s window boundary — reliably present with a longer window; U3/U3.5 untouched); `check`
  both arches; aarch64 virt v2 + GICv3 JM5 SMP 3/3 unchanged.
- **Tested — metal:** none this arc — U4 is fully QEMU-verifiable (every piece is
  scheduler/syscall logic: the handle install/resolve/clear, the owner-scoped reap, the
  two-child spawn/reap, and the `-ECHILD` negative are all deterministic under QEMU raspi4b —
  the reap wake is a scheduler post, not the timer). The child disk-loads ride the same EMMC2
  path M7 already metal-confirmed; a future reflash would show the two extra `hello from EL0`
  off the real card, but nothing in U4 is metal-*gated*.
- **Commit:** this arc on `hw-pi4` (merge pending the integrator, who records the merge hash).

### M7 — a minimal process model: sys_spawn + sys_wait (aarch64) ✅ `hw-pi4`
- **What:** the reaping half of a process model — an EL0 program can now spawn a child
  program and reap it. **`SYS_SPAWN` (8)** loads the fixed on-disk `HELLO.BIN` into a
  fresh per-task slot and runs it at EL0 as a *child*, returning its pid; **`SYS_WAIT`
  (9)** blocks the caller until that child exits and returns its exit status. A small
  static process table (cap 4) carries, per child, a counting `Semaphore` the child
  posts once (on exit *or* kill) and the parent waits once — a **scheduler** wake, so
  the reap is QEMU-testable, not timer-gated. The child's disk load reuses the M6g
  loader core, refactored into a shared, silent `load_program_into_slot()` (the M6g
  loader reconstructs its own lines from the result — its output stays byte-identical).
  The pid-recording race is closed by a co-location invariant (child queued on the
  caller's core, undispatchable until the parent yields in `sys_wait`), so **no
  scheduler change** was needed. The Pi pioneers roadmap-U4, as it did M6a–M6g.
- **Tested — QEMU:** `./arroyo kernel8-test 30` → after the M6g lines: `:: M7: process
  model — sys_spawn + sys_wait (parent reaps a disk-loaded child) ::`, a **third** `hello
  from EL0` (the M7 child), `:: M7: parent spawned child pid=<p>, waited, child exited
  status 0 -> PASS ::`, with every M6b/M6c/M6d/M6e/M6f/M6g + CAPSTONE line byte-identical
  (hex/pid-normalized set-diff: only the two new M7 marker lines + `hello` 2→3) and 0
  unexpected faults. x86 (`test` + `UNAOS_FATIMG=1 test`) functionally byte-identical
  through the U-lines (seam is `baremetal`-only; sole diffs are QEMU timer-calibration
  jitter); `check` both arches; aarch64 virt v2 + GICv3 JC2 SMP 3/3 unchanged.
- **Tested — metal (real Pi 4, 2026-07-04):** ★ PASS on silicon. `:: M7: parent spawned
  child pid=41, waited, child exited status 0 -> PASS ::` — the parent `sys_spawn`ed a child
  that loaded `HELLO.BIN` off the **real** card via the EMMC2-first path QEMU cannot exercise
  (`SD card @0xfe340000 — 31116288 blocks (15193 MiB, CSD v2)`), printed the **third** `hello
  from EL0`, and exited status 0; the parent's blocking `sys_wait` was woken by the child's
  scheduler post and reaped it — all under a live timer (EL0 preemption live: M6e
  `IRQs-taken-at-EL0=23`, M6f `spsentinel=3`). Full battery green on metal: M6b `exited=1
  killed=3 PASS`, M6d ×3, M6f ×3, M6g `disk-loaded EL0 program exited ok -> PASS`, CAPSTONE
  6/6, EL=1/CNTFRQ=54 MHz, **0 unexpected faults, 0 FAIL lines**. (Prepped non-destructively by
  swapping `kernel8.img` on the mounted FAT volume — no re-flash needed; the gate-verified
  binary.) Log: `target/serial-pi.m7-metal.log`.
- **Commit:** this arc on `hw-pi4` (merge pending the integrator, who records the merge hash).

## Round 6 — 2026-07-04 (landed on track branches; awaiting integration)

### U3 — per-process address spaces (CR3) (x86) 🔬 `hw-rmbp`
- **What:** each ring-3 process now runs in its OWN top-level page table (its own
  CR3) instead of sharing one user window — the x86 mirror of aarch64 M6d. A static
  8-slot pool of page tables each SHARES the kernel half (copies every kernel PML4
  entry except the user-window slot, so the identity map / MMIO / heap / kernel
  stacks are shared) and owns a PRIVATE user window at USER_BASE. The scheduler
  installs a task's CR3 before dropping to ring 3 and restores the kernel CR3 +
  frees the slot on exit. Two processes can map the same address to different
  memory. Plain `mov cr3` (full TLB flush) — PCID (the x86 ASID analogue) deferred.
- **Tested — QEMU:** `./arroyo test 25` → a deterministic kernel isolation probe
  (two spaces, same VA, distinct sentinels, swap CR3 and read → distinct) PASS, and
  two ring-3 tasks each in their own CR3 each read their own private sentinel PASS;
  U1a/U1b/U2/U2.5 byte-identical, no reboot loop; coexists with the U2 disk loader
  (`UNAOS_FATIMG=1`), the U2.5 FTDI console (`UNAOS_USBSERIAL=1`), and the FAT
  regression (`test-fat part`/`sf`). `./arroyo check` both arches; 0 aarch64 files.
  4-lens adversarial review → 0 confirmed findings.
- **Metal — PENDING:** rides the next rMBP reflash (FTDI cable day, ~2026-07-08).
- **Commits:** on `hw-rmbp` (see landing report); unmerged (Fable credits out).

---

## Round 5 — 2026-07-03

### M6g — load a program from storage (aarch64) ✅ `hw-pi4`
- **What:** the Pi twin of x86 U2 — the first *program loaded from the microSD the
  Pi booted from* into the EL0 boundary. A block-layer backend seam lets the
  read-only path dispatch to a new BCM2711 EMMC2/SDHCI microSD driver (PIO,
  single-block CMD17, polled, no writes) beside the untouched xHCI path; the driver
  probes **EMMC2 first, legacy Arasan second** (the reverse of QEMU, so the metal
  base is the first tried). The loader mounts the card's FAT volume, reads
  `HELLO.BIN`, size-checks it, copies it into a fresh per-task M6d slot (EL0-RX/EL1-RO
  before the task exists), and runs it at EL0 (`hello from EL0`). The loaded bytes are
  untrusted — bounded only by size, contained by EL0 + per-page perms + the M6b
  fault-kill net.
- **Tested — QEMU:** `./arroyo kernel8-test 30` → `SD card @0xfe300000 identified —
  131072 blocks (64 MiB, CSD v1)`, `FAT mounted from SD (Fat32)`, `HELLO.BIN loaded
  from SD (51 bytes) -> EL0`, second `hello from EL0`, `disk-loaded EL0 program exited
  ok -> PASS`, with every prior milestone byte-identical and 0 unexpected faults; the
  `UNAOS_SDIMG=0` no-SD control adds exactly the two no-card lines + the loader-skipped
  line. x86 (`test` + `UNAOS_FATIMG=1 test`) byte-identical (seam inert); `check` both
  arches; aarch64 virt v2 + GICv3 JC2 SMP 3/3 unchanged.
- **Tested — metal (real Pi 4, 2026-07-04):** ★ the driver's EMMC2-first success leg —
  the one QEMU physically cannot exercise — ran on silicon: **no fallback line**, `SD card
  @0xfe340000 identified — 31116288 blocks (15193 MiB, CSD v2)` (the real ~16 GB microSD,
  SDHC/block-addressed — vs QEMU's 64 MiB/CSD v1 legacy fallback), then `FAT mounted from
  SD (Fat32)`, `HELLO.BIN loaded from SD (51 bytes) -> EL0`, second `hello from EL0`,
  `disk-loaded EL0 program exited ok -> PASS`. This reflash also carried M6f's pending
  metal: all three M6f verdicts PASS and the per-task preempt rider went > 0 on silicon
  (`spsentinel=3`); M6b `exited=1 killed=3 PASS`, M6d ×3 PASS, M6e `IRQs-taken-at-EL0=26`,
  CAPSTONE 6/6, 0 unexpected faults. EL=1, CNTFRQ=54 MHz.
- **Commit:** `faad571` (merge) · arcs `11b8191` `a072a48` `683d48c`.

### U2.5 — FTDI USB-serial console (x86) 🔬 `hw-rmbp`
- **What:** a captured console for the serial-less 2012 rMBP. The kernel
  enumerates an FTDI FT232 (0403:6001) on the xHCI bus, configures it
  (115200 8N1), and drains a 64 KiB boot-capture ring — fed by every
  `serial_print!` since the first — out its bulk-OUT endpoint, so the whole early
  boot log replays out the cable. Also folds three U2-review hardening items
  (first-entry x87/MMX scrub, per-CPU `DR7` clear, whole-ring-3-window zero before
  a load) and fixes the APIC ms-clock: the BSP heartbeat now re-arms *after* the
  calibrated rate is stored (it was pinned to the fixed fallback → metal read
  ~119 Hz instead of ~1000).
- **Tested — QEMU:** `UNAOS_USBSERIAL=1 ./arroyo test 25` → `FTDI USB-SERIAL
  DETECTED (0403:6001)`, `FTDI console up`, `FTDI TX mirror -> PASS (~17 KB
  replayed)`, `target/ftdi.log` carries the boot log; coexists with the U2 disk
  loader (`UNAOS_USBSERIAL=1 UNAOS_FATIMG=1` → both PASS) and the usbdebug view.
  No-knob boot log byte-identical but for the `DR7 cleared` line and the APIC
  re-arm. FAT regression (`test-fat part`/`sf`) green; `./arroyo check` both arches.
- **Metal — PENDING (~2026-07-08):** rides the physical FTDI cable (B0CJVC19CF).
  QEMU-only until then; the APIC ~1000 Hz truth and the FTDI console verify on the
  real rMBP on cable day (`UNAOS_USBDEBUG=1` boot, FTDI in a root USB-A port).
- **Commits:** `229d675` (Part 0 folds) · `ab0f975` (APIC re-arm) · `f7c929e` (FTDI console).

### U2 — loadable ring-3 programs from FAT (x86) ✅ `hw-rmbp`
- **What:** the first *real program loaded from disk* into the x86 privilege
  boundary. A flat ring-3 binary (`HELLO.BIN`) is read off a FAT volume,
  validated, copied read-only-from-start into the user code page, and run in
  ring 3 (`hello from disk`). Plus the boundary preconditions that make loading
  untrusted code safe: #DB and #MC on dedicated interrupt stacks (closing a
  user-triggerable CPU-halt), a register scrub at first entry, and self-test
  fixtures for the NMI stack and the CVE-2012-0217 guard.
- **Tested — QEMU:** `UNAOS_FATIMG=1 ./arroyo test 25` across all four FAT
  layouts → the three U2 lines + the Part-0 fixtures + U1a/U1b still PASS, full
  USB boot, 0 unexpected faults. `./arroyo check` both arches.
- **Tested — metal (real 2012 MacBook Pro, 2026-07-03):** Realtek USB3 SD reader
  → FAT16 card → `HELLO.BIN` (72 B) → `hello from disk` PASS. *Metal-pending:*
  the #DB-resume path (TCG can't model the single-step-on-SYSCALL trap) and the
  #MC fire path.
- **Commit:** `9cdf397` (merge) · arc `7d8a6bb`.

### M6f — validated user pointers + wider syscalls (aarch64) ✅ `hw-pi4`
- **What:** `copy_from_user`/`copy_to_user` (bounds- and overflow-checked; a bad
  pointer is an error return, not a task kill) plus the first "real" syscalls
  (`yield`, `sleep_ms`, `getpid`, `getinfo`). Also folds five hardening items
  from the M6d review, incl. scrubbing the FP/SIMD registers at first entry.
- **Tested — QEMU:** `./arroyo kernel8-test 30` → the M6f fixtures PASS
  (getinfo round-trip; four hostile pointers all refused with no kills;
  yield/sleep interleave) with every prior milestone still green.
- **Tested — metal (real Pi 4, 2026-07-04, on the M6g reflash):** all three M6f
  verdicts PASS on silicon (getinfo/copy_to_user round-trip, 4 hostile pointers
  refused with 0 kills, yield/sleep interleave) and the per-task EL0 preempt rider
  went > 0 (`spsentinel=3`, QEMU shows all 0) — the timer preempted that slot task
  and it resumed correctly under its own ASID.
- **Commit:** `ee21e30` (merge) · arcs `71ed153` + `e65ffc0`.

### JM2 — Orin headless first light (aarch64/Jetson) 🔬 `hw-jetson`
- **What:** makes the Jetson build boot headless and safe. Gates the QEMU-virt
  SMP path off the Tegra build (it would otherwise touch Tegra memory that
  isn't there), makes the bootloader boot without a display (the shared
  bootloader — the MacBook boots through it too), and adds a boot-diagnostics
  knob that reports the firmware's real serial/display configuration.
- **Tested — QEMU:** full battery byte-stable — `./arroyo check` both arches;
  x86 U1a/U1b PASS; aarch64 virt v2 + GICv3 SMP lines intact; Pi `kernel8-test`
  unchanged. The Tegra feature is off in all of these, so nothing regresses.
- **Tested — metal (real Orin Nano, 2026-07-03):** the boot diagnostics ran on
  silicon (genuinely headless firmware: 0 display handles) and the headless
  path **entered the kernel for the first time on Orin** — which then faulted
  on its first Tegra UART register read because the firmware-handoff page
  tables don't map Tegra device memory. That diagnosis is the next arc (JM3:
  kernel-owned MMU).
- **Held at integration review:** the merge is waiting on a small must-fix —
  the boot-diagnostics DTB table scan compares against a wrong GUID constant
  (it can never match), so the captured "firmware publishes no DTB" line is
  withdrawn as unverified until a re-run with the corrected constant. Fix
  rides at the head of the JM3 arc.
- **Commit:** *(merge pending the fix)* · arcs `811259c` `d382677` `0bd0dae`
  `d0835c0` `27bf835`.

---

## Round 4 — 2026-07-02

### M6d — per-task address spaces + ASIDs (aarch64) ✅ `hw-pi4`
- **What:** every user task gets its own isolated page tables and its own stack,
  ASID-tagged so task switches need no TLB flush. This is what lets two programs
  use the same virtual address for different data — real process isolation.
- **Tested — metal (real Pi 4):** same-VA isolation proven distinct on real A72
  TLBs (QEMU can't test this — it has no TLB model), stack write/readback PASS,
  all under live timer preemption. `4a06a8c`.

### JC2 — PSCI SMP on GICv3 (aarch64/Jetson) 🔬 `hw-jetson`
- **What:** brings up secondary CPU cores on the QEMU-virt GICv3 path via PSCI,
  proven by cross-core interrupts (each core pinged, both directions).
- **Tested — QEMU:** `UNAOS_GICV3=1 ./arroyo test-arm 30` → 3 cores online +
  cross-core SGI both ways; v2 single-core and Pi SMP unchanged. Metal (Orin)
  pending. `18be259`.

## Round 3 — 2026-07-02

### U1b — ring-3 fault isolation + boundary hardening (x86) ✅ `hw-rmbp`
- **What:** a faulting ring-3 program is killed (kernel survives); plus register
  scrubbing, the CVE-2012-0217 guard, an NMI stack, and cross-core W^X.
- **Tested — metal (real 2012 MacBook Pro):** SMEP active, 3 fault-kills with
  correct syndromes, kernel alive past all three. `37d2af8`.

## Round 2 — 2026-07-02

### M6e — preemptible EL0 (aarch64) ✅ `hw-pi4`
- **What:** the timer can preempt a running user task and resume it correctly.
- **Tested — metal (real Pi 4):** 18 preemptions of a spinning EL0 task, all
  resumed correctly. `e62fd4c`.

## Round 1 — 2026-07-02

First rotation of the three-track machine: **U1a** (x86 ring-3 round-trip),
**M6c** (aarch64 loadable blob), **JC1** (GICv3 beside GICv2) — all landed,
reviewed, merged. `637ee5c`.
