# UnaOS — Real-Hardware Bring-up (video stack)

How to boot this branch (`c01-04_k01-04_vid-stack`) on real hardware and what to expect. The goal
of this branch is **video on metal**: a UEFI-GOP framebuffer at the monitor's native resolution,
an on-screen boot log, a red panic screen, and a double-buffered GUI console.

There is **no serial port** on these machines, so the on-screen `fbcon` log *is* the debug output.
Take a photo of the screen to capture results.

## What works on metal (this branch)

- **Video:** UEFI GOP framebuffer; resolution auto-detected and set to the monitor's **native**
  resolution via **EDID** (falls back to the firmware's current mode if EDID isn't exposed —
  never the GPU's max). Double-buffered GUI, damage-tracked, native-res-aware.
- **On-screen diagnostics:** every boot message is mirrored to the framebuffer (`fbcon`) until the
  GUI takes over; a kernel panic paints a red screen with the message.

## What is NOT on this branch (so you know what to expect)

- **xHCI BIOS→OS handoff** lives on `c01-02`, not here. On the Mac the firmware/SMM may keep
  ownership of the USB controller, so **the USB keyboard may not work** — you'll see video but may
  not be able to type. That's fine for verifying *video*. (Typing needs the `c01-02` USB work.)
- Networking (`c01-03`) and SMP (`c01-02`) are on other branches.
- The Pi VideoCore **mailbox** driver (bare-metal, no UEFI) isn't built yet — the Pi uses UEFI
  (Path A) below.

---

## Target A — MacBook Pro Retina (mid-2012, x86-64 UEFI) — fastest video win

### 1. Build the boot media
```sh
cd unaos
./arroyo esp-x86      # builds kernel + bootloader, packages target/x86_64_esp/ (no QEMU)
```
This produces `target/x86_64_esp/` containing `EFI/BOOT/BOOTX64.EFI` and `kernel.elf`.

### 2. Write it to a USB stick (FAT32 / "MS-DOS")
A Mac boots any FAT USB that has `/EFI/BOOT/BOOTX64.EFI`. Format the stick as MS-DOS (FAT) in Disk
Utility, or from the terminal (replace `diskN` — check `diskutil list` carefully!):
```sh
diskutil eraseDisk MS-DOS UNAOS MBRFormat /dev/diskN
cp -R target/x86_64_esp/EFI /Volumes/UNAOS/
cp target/x86_64_esp/kernel.elf /Volumes/UNAOS/
diskutil eject /Volumes/UNAOS
```
Result on the USB: `/EFI/BOOT/BOOTX64.EFI` and `/kernel.elf`.

### 3. Boot
- Insert the USB, power on holding **⌥ Option**, pick **"EFI Boot"**.
- Expect: the `fbcon` boot log scrolls, then the **GUI console** (dark-blue background, green
  `architect@unaos:~$` prompt) at the **panel's native resolution**.
- The boot log shows: the GOP mode list, the `EDID: monitor native resolution WxH` line, and the
  `GOP: ...` decision. Photograph it — that tells us the panel's real stride/format/native res.

### Watch for
- **`framebuffer_size < stride*height*bpp` warning** — flags an Apple-GOP stride quirk (the #1
  untested risk). If video looks sheared/offset, this is why; send me the logged numbers.
- **Hang during boot** — the last line on screen says where (likely PCI/xHCI). Photograph it.
- **No keyboard** — expected (see above); video is still the test.
- 2012 Macs don't enforce Secure Boot for USB EFI boot, so no firmware change is needed.
- Recover: hold the power button to force off, remove the USB.

---

## Target B — Raspberry Pi 4 B (aarch64) — UEFI (Path A)

The Pi doesn't boot UEFI on its own; put the **Tianocore RPi4 EDK2 UEFI** firmware on the SD card,
which then loads our bootloader → GOP → reuses the whole video path.

### 1. Build the boot media
```sh
cd unaos
UNAOS_SKIP_XHCI=1 ./arroyo esp-arm   # packages target/aarch64_esp/ (BOOTAA64.EFI + kernel.elf)
```
Use `skip_xhci` for the video bring-up (same as the Mac): video is pure GOP and needs no USB/PCIe,
and skipping xHCI avoids any controller stall and keeps the (still-buggy) aarch64 DTB/ECAM path out
of the picture. Add `UNAOS_BOOTLOG=1` to hold the boot log on screen and read the EDID/mode line.

### 2. SD card (FAT32, tested against pftf/RPi4 **v1.52** — https://github.com/pftf/RPi4/releases)
1. **Extract the ENTIRE firmware zip to the FAT32 root and rename nothing** — everything is bundled
   (`RPI_EFI.fd`, `config.txt`, `start4.elf`, `fixup4.dat`, `bcm2711-rpi-4-b.dtb`, `overlays/`,
   `firmware/`). Nothing needs fetching separately from raspberrypi/firmware.
2. Copy our ESP onto the same partition: `target/aarch64_esp/EFI/BOOT/BOOTAA64.EFI` (the firmware
   auto-launches the removable-media default path `\EFI\BOOT\BOOTAA64.EFI`) and `kernel.elf` at the
   SD root (our bootloader opens `kernel.elf` from its own boot volume).

### 3. Boot
- HDMI + power. The Pi firmware → EDK2 UEFI → our `BOOTAA64.EFI` → GOP at the HDMI monitor's native
  resolution → GUI (dark-blue, green prompt). Photograph the screen (no serial, same as the Mac).
- **For video you do NOT need to change any firmware setting** — the default **ACPI** mode is fine
  (GOP/HDMI is independent of the DeviceTree). The firmware's EDID-over-HDMI may actually feed our
  EDID parser here (it didn't on Apple EFI); a `UNAOS_BOOTLOG=1` build will show `EDID read: source=…`.
- **No USB keyboard** on this `skip_xhci` build (expected). Enabling USB later needs a full build,
  the firmware switched to **Devicetree** mode (Device Manager → Raspberry Pi Configuration →
  Advanced), and the bootloader **DTB-GUID fix** (the hardcoded `EFI_DTB_TABLE_GUID` is currently
  wrong, so `dtb_addr` is never found) — all deferred to the USB/PCIe phase, none needed for video.

### Path B (later)
Bare-metal `kernel8.img` + the BCM2711 VideoCore mailbox framebuffer driver — not built yet; needs
a bare-metal (no-UEFI) aarch64 boot path. Tracked for a future session.

---

## Reading the results

On metal there's no QMP screendump — the screen itself is the output:
- **GUI console with a prompt at native res** → video bring-up succeeded.
- **Red screen with a message** → kernel panic; the message + location are on screen.
- **Frozen boot log** → it hung; the last line is the clue.

Photograph the screen (especially the GOP/EDID log lines and any warning) and send it over.
