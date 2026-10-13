# DONGLEBOOT — rmbp-ledger B658

Flight 35 boot 2 (`docs/dev/evidence/rmbp-0915/flight35/f35-boot2.log`, the USB NIC plugged at power-on): login
21:58:04Z, `[boot] services up first=bt held_starters=8`, `[desktop] starter after-bar who=usbnet waited_ms=10004
why=timeout`, then at 21:58:42Z:

```
[lock] held-too-long name=prefs.rs:224 at=prefs.rs:443 by=36@cpu2 ms=37522 waiters=0
[lock] held-too-long name=ahci.rs:249 at=ahci.rs:680 by=36@cpu2 ms=37519 waiters=2
[lock] spin-wait name=ahci.rs:249 at=ahci.rs:680 waiter=12@cpu1 on=36@cpu2 ms=37410
```

and no `[desktop] built` before the reboot. Status: compiled, not flown. Nothing below has run on QEMU or on the bench.

## 1. Why a prefs stat reaches AHCI (read from the code and the three boot logs)

* The stat does NOT walk mounts. `MountTable::resolve` (fs/vfs.rs:567) is longest-prefix; `<home>/settings/<d>`
  resolves to `/`, the native UnaFS on the card (`[vfs] root mount / = native unafs volume source=sdhc`). The EFI
  volume of `ahci0` (`[vfs] volume mounted /volumes/EFI source=ahci0 rw=no`) is withdrawn from the table on a native
  root (fs/volumes.rs `withdraw_foreign_esp`, `[vfs] volume /volumes/EFI withdrawn`), and the dongle reorders no
  volume.
* The TABLE does. `watch_deleted` (prefs.rs:443-445) built `crate::shell::vfs_mount_table()` every pass (2 s,
  `WATCH_EVERY_MS`). That builder (shell.rs:7444) runs `fs::bootdisk::bind` → `bind_root(mt, found.source,
  unafs_state(found.source), …)` (fs/bootdisk.rs:1434) → `unafs_state` → under `ahciroot` (on the image line since
  INSTALL4, `unaos/scripts/image-line.env`) `install::ahciroot::ensure_root_grant()` (fs/bootdisk.rs:1292). That
  function caches only a POSITIVE answer (install/ahciroot.rs:37 "Positive answers only are cached", :69) and
  otherwise walks every AHCI disk through `ours_unafs_part` → `selfinstall::probe` + `partition::census` — sector
  reads on the SSD. The bench SSD carries macOS, never OURS, so EVERY table build re-reads it.
* The wire agrees on all three boots, dongle or not: the same pair at 53-99 ms on every watch pass
  (`f35-boot1.log` 21:24:01Z `prefs.rs:224 at=prefs.rs:443 … ms=83` then `ahci.rs:249 at=ahci.rs:680 … ms=93`;
  boot 3 22:00:15Z on). The dongle is not on this path.

## 2. Why one AHCI hold was 37.5 s against a 1 s bound

* `read_block_at` (ahci.rs:676 before this arc) issues ONE command polled by `wait_ms` on the TSC against
  `T_CMD_MS = 1000` (ahci.rs:186, :205). The TSC runs whatever xHCI does, and `issue` does not retry. No
  `[ahci] … TIMED OUT` line printed in boot 2, so the command completed inside its bound.
* The 37.5 s is the HOLDER OFF ITS CORE. `PORTS` is a plain spin `crate::sync::Mutex` held with interrupts ON; task
  36 is `prefs-watch`, a preemptible kernel task on worker cpu2 (prefs.rs `service`, `spawn_stack("prefs-watch",
  …, cpu, cpu, …)`). In the same second (21:58:04Z) `:: [smpbal] steal 'bg-user' c1->c2 (m=1) ::` moved the login's
  ring-3 HOLOCRON (pid 35) onto cpu2; a default ring-3 task is cooperative (IF clear, runs to completion —
  arch/x86_64/sched.rs:208, :482). Boot 2 never prints `[holocron] ring=opened`. The holder ran again at
  21:58:42Z. Boot 3's steal went `c1->c0`, `[holocron] ring=opened … ms=1247`, `[desktop] built … bar_ms=340`.
  The steal is GATESTEAL's (sched.rs, not this arc's); the hold across a preemptible window is ahci.rs's.

## 3. The fix (this commit)

* **ahci.rs — the hold bounded.** `dongle::with_port` (ahci.rs:1142) is now the only way `PORTS` is held across a
  command (the read at :688/:693, the `ahci-write` write at :972). It takes and releases the lock with interrupts
  MASKED, so the holder cannot be preempted inside the hold; it ACQUIRES with a bounded try (`T_CMD_MS +
  T_PORT_MS`), never an unbounded spin (`[ahci] busy refused …`); the render task never waits at all (`[ahci]
  render refused …`, the caller reads an unreadable sector). A command that misses its bound marks the port and
  stops it, no restart and re-poll (ahci.rs:548-566): `[ahci] cmd timeout port=N cmd=0x.. ms=<n> bound_ms=1000
  … -> port marked`, and every later command on it is refused at `issue`'s head (:481). Worst case per dead port,
  once: `T_CMD_MS + T_PORT_MS` masked. `flush_at` (10 s bound) keeps its unmasked hold; a masked taker gives up on
  it at the bound instead of spinning.
* **prefs.rs — the store's table.** `store_mt` (prefs.rs:1304): once the root is bound and the store's path is
  claimed by `/` alone on a native root (decided once per boot from one full table), every store read, write and
  stat resolves against ONE mount — `/` as `NativeBackend::new("native")`, the backend `bootdisk::bind_root` mounts
  there. Folded onto the existing lines (prefs.rs:177, :190, :208, :295, :373, :444, :505). A FAT root keeps the
  full table. `[prefs] stat domain=<d> mounts_walked=<n> device=<handle> store=<native-root|full-table>` once per
  boot at the watch's first pass.
* **The render task on a block device.** Any render-task table build reads the SSD the same way (§1); the first
  candidate on this boot is the build's wallpaper re-arm, probed on the composite's flush (desktopbuild.rs
  `service`) — named from the code, not from a line. It is made non-blocking at the driver (above). The probe
  itself is ahciroot.rs's; see §5.
* **desktopbuild.rs — the starter hold.** `built()` (desktopbuild.rs:37, `strip::compose_all`'s gate on every
  composite) stamps the render task's liveness. `steps::release_why` (:302): a held pump starter is released
  `why=render-wedged` when the build is in flight (past its stage wait, owed or its first paint pending) and the
  render task has not composited for `RENDER_WEDGE_MS = 3 x PAINT_WAIT_MS = 1500 ms` — not at the 10 s bound.
  `[desktop] starter after-bar who=usbnet waited_ms=<n> why=render-wedged render_quiet_ms=<n>`.

## 4. The fixture — `tests dongleboot` (desktopbuild.rs:381, registered in tests.rs:191)

```
[prefs] stat domain=<d> mounts_walked=1 device=<handle> stats=<n> store=native-root
[ahci] cmd timeout port=N cmd=0x25 ms=1000 bound_ms=1000 forced=1 … -> port marked (DONGLEBOOT: …)
[ahci] port N marked — command 0x25 refused at once (DONGLEBOOT: …)
[ahci] render refused ix=0 lba=0 n=1 (DONGLEBOOT: …)
:: DONGLEBOOT: prefs_stat=home-only ahci_bound=ok(<hold ms>) render_spin=0 starter=released -> PASS :: device=<handle> wedge_ms=1500 timeouts=1 render_refused=1 busy_refused=0 max_hold_ms=<n>
```

Legs: (1) the watch's stats after one warm call, zero `read_block_at` calls across the pass and one mount walked;
a FAT root reads `skip(fat-root walked=N)`. (2) one READ on the first AHCI disk whose poll ignores completion (the
REAL 1000 ms bound fires), the whole hold within `T_CMD_MS + T_PORT_MS`, the next read refused in under 1 ms, the
port restarted and unmarked and reading again; no disk = `skip(no-disk)`. (3) the fixture holds `PORTS` and reads
as the render task: refused with 0 ms of spin. (4) the release rule's six cases. QEMU shape: `-device usb-net` and
an AHCI disk (q35's `ahci0`); x86 `wc` + `witness`; `ahci` for legs 2-3.

## 5. Owed outside this arc's files (STOP-reported, not made)

* **install/ahciroot.rs `ensure_root_grant` (:69) caches the negative.** The exact change: a `static ROOT_NONE:
  AtomicBool`; after the `for ix in 0..block::MAX_AHCI_DISKS` loop finds no OURS disk, `ROOT_NONE.store(true,
  Release)`; at the top, after the `root_port()` early return, `if ROOT_NONE.load(Acquire) { return None; }`; the
  installer's successful `install ssd --write` (and `format_fresh`) clears it. With it no table build reads the SSD
  after the first; without it every table build (render's included) still reads the SSD's GPT, bounded now.
* **GATESTEAL (sched.rs):** a cooperative ring-3 task stolen onto a core whose kernel task holds ANY spin lock
  starves that holder (here `prefs.rs:224` SAVE_LOCK too, 37522 ms). This arc bounds the AHCI hold; the steal is
  GATESTEAL's.
* The build in boot 2 printed no `[desktop] built` at all, so whether it got past its stage wait is not on the
  wire; `why=render-wedged` needs it past the stage wait (`in_flight`), else the 10 s bound stands.

## 6. The wire the next flight should show (dongle at power-on)

* `[prefs] stat domain=… mounts_walked=1 device=sdhc store=native-root` once; no `prefs.rs:224`/`ahci.rs:249`
  held-too-long pair on the watch passes.
* No `held-too-long name=ahci.rs:249` above the command bound; no `spin-wait … ahci.rs` by the render task (at most
  `[ahci] render refused` lines).
* `[desktop] built at=login ok …`; the usbnet starter `why=settled`, or `why=render-wedged` inside ~1.5 s.
