# Debugging UnaOS on real hardware

How to get a console off each metal target. QEMU is the fast loop (`./arroyo
test` / `test-arm` / `kernel8-test`, serial → `target/serial*.log`); this doc
is for **on-silicon** bring-up, where the hard, QEMU-invisible bugs live.

> Read serial logs with `awk '/pattern/' <log>` or `grep -a`, **never plain
> `grep`** — control bytes in the logs break it.

Common shape across all three: build boot media with an `./arroyo` target,
capture a console from byte 0, compare against the QEMU log for the same build.

> **Default-quiet boots vs. the fixture battery (`witness`).** A *default* boot
> — every media/boot target (`x86`/`arm`/`esp-x86`/`esp-arm`/`esp-jetson`/
> `kernel8`/`vm-image`) — reaches the shell with **boot-honesty lines only**:
> live hardware state discovered *this* boot (device found, link state, the APIC
> calibration line, the SD-card census, `MISSION SUCCESS`, the `CAPSTONE`
> scheduler terminus, the net-stack liveness witnesses). The battery of EL0/kernel
> **fixtures that re-prove long-metal-confirmed facts** on every boot (x86:
> `U2-0c` self-NMI/canonical-guard + `U1a`/`U1b`/`U2-0a`/`U3`/`U3.5` + the
> `U2`/`U4x`..`U6bx` storage chain that cascades `U7x`..`U6gx` + the ring-3
> `SOCK-*` syscall fixtures; aarch64 bare-metal: `M6b`/`M6e`/`M6d`/`M6f`/`M6g`
> + `U4`..`U7`, which cascades `U9`/`U10`/`U10c`/`U10d`/`U11`/`U11-defer`/
> `U11-reap` + the `K*`/`F2`/`F3`/`BANDY`/`unafs` selftests) is gated behind the
> kernel's **`witness`** cargo feature, **default OFF**. The QEMU regression
> commands **auto-arm it**: `./arroyo test` / `test-fat` / `test-arm` /
> `kernel8-test` export `UNAOS_WITNESS=1` (which arroyo turns into the `witness`
> feature, and the x86 `builder` re-derives the same from the env), so their
> witness coverage is identical to before — same `-> PASS` tally, same `CAPSTONE
> COMPLETE`, `0 FAIL`. To arm the fixtures on a *boot/media* build (e.g. to watch
> the whole battery on metal), set `UNAOS_WITNESS=1` explicitly:
> `UNAOS_WITNESS=1 ./arroyo kernel8`. On aarch64 `virt`/`tegra` the `witness`
> feature gates nothing (all its call sites are x86- or bare-metal-scoped), so
> arming it for `test-arm` / leaving it off for `esp-jetson` are both no-ops for
> the emitted code — the jetson media stays witness-free by construction. Gates
> **call sites only**, never fixture bodies. See
> `review/unaos-default-quiet-LANDING.md`.

---

## `hw-rmbp` — 2012 MacBook Pro (x86_64)

**The catch: no hardware serial port.** The FTDI USB-serial console driver has
landed **QEMU-green** (see below) but is **metal-pending** until the physical
FTDI cable arrives (~2026-07-08); until then the console is still the
**framebuffer**, captured by **phone photo**. `serial_print!` is mirrored to
fbcon so kernel output is visible on the panel — and, from the very first print,
into the FTDI boot-capture ring so the whole early log replays out the cable the
moment the console comes up.

- **Build boot media:** `./arroyo esp-x86` writes an ESP; flash it to a USB
  stick (or use the builder's image) and boot the Mac holding **Option**.
- **See the pre-GUI half** (SMEP / U1a / U1b / the U2 loader): the GUI detaches
  fbcon before the main loop, so use `UNAOS_USBDEBUG=1 ./arroyo esp-x86` — it
  keeps the debug view up and runs the U2 FAT loader in it.
- **Knobs:** `UNAOS_USBDEBUG=1` (debug console view), `UNAOS_BOOTLOG=1`
  (verbose boot), `UNAOS_SKIP_XHCI=1` (skip USB bring-up when isolating),
  `UNAOS_SCHED_DEMO=1`, `UNAOS_USBSERIAL=1` (attach an emulated FTDI console —
  see the FTDI section below).
- **Storage note:** the U2 loader mounts the **USB SD-card reader** as its block
  device — put `HELLO.BIN` on that card's FAT root. The internal boot dongle
  enumerates but never becomes a block device (a known wrinkle).
- **Loop:** build → flash → boot → photograph the panel → compare to the QEMU
  `target/serial.log` for the same build.

### FTDI USB-serial console (U2.5)

The kernel enumerates an FTDI FT232 (VID 0x0403 PID 0x6001) on the xHCI bus,
configures it (115200 8N1), and drains a 64 KiB boot-capture ring out its
bulk-OUT endpoint — every `serial_print!` since the very first is mirrored into
that ring, so when the console comes up mid-boot the whole early log replays out
the cable. This finally gives the board a real captured console (no more phone
photos) and truthful timestamps (the APIC ms-clock re-arm fix rides this arc).

- **QEMU (now):** `UNAOS_USBSERIAL=1 ./arroyo test 25` attaches QEMU's
  `-device usb-serial` (an FT232 emulation) with its chardev backed by a file;
  the replayed console lands at **`target/ftdi.log`**. Inspect it with
  `awk`/`grep -a` (never plain `grep` — control bytes). Look for
  `>>> FTDI USB-SERIAL DETECTED (0403:6001) <<<`,
  `:: U2.5: FTDI console up (0403:6001, 115200 8N1) ::`, and
  `:: U2.5: FTDI TX mirror -> PASS (<n> boot bytes replayed) ::`. NOTE: the knob
  widens qemu-xhci to `p2=8,p3=8` root ports — with the default 4 USB2 ports the
  4th USB2 device (storage+kbd+tablet+serial) overflows onto an auto-inserted hub,
  and hub-downstream enumeration is HID-only this arc, so the FTDI would never be
  configured.
- **Metal (pending, ~2026-07-08):** boot `UNAOS_USBSERIAL` is a QEMU-only knob —
  on metal the driver simply enumerates whatever FT232 is plugged in, so the
  cable-day recipe is `UNAOS_USBDEBUG=1 ./arroyo esp-x86` (the usbdebug view keeps
  the console path live and shows the pre-GUI half) with the FTDI cable plugged
  **directly into one of the two root USB-A ports** — displacing the keyboard/mouse
  or the SD reader for the session. **This supersedes the old "needs a powered hub"
  note:** enumeration is *root-port only* this arc, so behind a hub the driver
  never sees the cable and the verify silently fails. Host-side capture command
  and the exact TTL pinout are **TBD on metal** (cable is USB-A FT232). FTDI RX
  (a real input console) and FTDI-behind-a-hub are future arcs.

---

## `hw-pi4` — Raspberry Pi 4 (bare-metal aarch64)

**Console: PL011 UART over a Raspberry Pi Debug Probe** (or any 3V3 USB-TTL),
115200 8N1, on the Pi's GPIO14 (TX) / GPIO15 (RX).

- **Build the image:** `./arroyo kernel8` produces `kernel8.img` (flat load at
  `0x80000`, no UEFI). Copy it to the microSD boot partition.
- **QEMU dry run first:** `./arroyo kernel8-test [secs]` boots the same image on
  `raspi4b` headless (serial → `target/serial-pi.log`). Note: QEMU raspi4b
  delivers **no Group-1 timer IRQ** and no SGIs on the `pi` build, so
  preemption/interrupt behavior is metal-only — that's the whole reason for the
  silicon loop.
- **Host TX on macOS needs the bridge:** macOS mangles raw TX to the probe; use
  the committed **`unaos/scripts/pi-serial-bridge.py`** (never `pkill -f cat`
  the port). It opens the probe cleanly for both directions.
- **Reflash loop:** `./arroyo kernel8` (image matches `HEAD`) → note `wc -c` of
  the current serial log as an offset → reseat the microSD, power-cycle → read
  the bridge from that offset. Batch milestones per reflash when the metal-only
  part warrants it.

---

## `hw-jetson` — Jetson Orin Nano (aarch64, GICv3)

**Console: a Raspberry Pi Debug Probe on the board's 3-pin UART TTL header**
(pin 3 = RX, pin 4 = TX), 115200. The USB-C debug port did **not** enumerate a
serial device on this dev kit, so the TTL header is the path.

- **Build boot media:** `UNAOS_TEGRA=1 ./arroyo esp-jetson` writes a single
  MBR FAT32 USB (carries `EFI/BOOT/BOOTAA64.EFI` + kernel). Add
  `UNAOS_BOOTDIAG=1` on the first boot of a new build to dump the firmware id,
  GOP handle count, ConOut path, and the DTB `/chosen` stdout-path (**the UART
  truth** — which Tegra UART the console actually is).
- **Boot:** insert the USB, power on; at the UEFI shell `connect -r` mounts the
  volume. The board runs GICv3 on silicon; the kernel is currently **headless**
  (no GOP without a native DP display — a passive DP→mini-HDMI adapter drives
  nothing; a native DP→DP or active adapter is required for graphical).
- **Tegra build stops early by design:** with `UNAOS_TEGRA=1` the kernel
  diverges before GIC/timer init (`:: tegra: early platform stop … ::`) and
  spins a serial heartbeat — that heartbeat *is* the liveness proof until the
  Orin GIC/timer arc (JM3) lands. Kernel dark after `ExitBootServices` ⇒ suspect
  the tegra UART base; **stop and report**, don't chase.
- **Host TX on macOS:** hold the serial fd open before `stty` (a macOS baud
  quirk); a bridge script (`unaos/scripts/jetson-serial-bridge.py`) handles it.

---

## When silicon disagrees with QEMU

That is the expected, valuable case — QEMU can't model caches, real interrupt
delivery, SMEP, or firmware memory maps. The discipline: **implement →
adversarial review aimed at the QEMU-invisible risk → QEMU-verify the structure
→ reflash and verify the metal-only half.** Record exactly what silicon showed
(the ledger in [`../SECURITY.md`](../SECURITY.md) marks each item QEMU-verified
vs metal-confirmed); if metal diverges from the brief, **stop and report** —
don't paper over it.
