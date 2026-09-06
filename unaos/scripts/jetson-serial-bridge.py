#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 The Architect & Una
#
# jetson-serial-bridge.py — bidirectional serial bridge for the Jetson Orin Nano console (macOS).
#
# The Orin is driven headless over serial. The board's TTL UART header (the rear "section 11" pins:
# pin 3 = board RX, pin 4 = board TX, GND<->GND) is wired to a Raspberry Pi Debug Probe, which
# presents on macOS as a single CDC-ACM device (/dev/cu.usbmodem*; VID 2e8a "Debug Probe _CMSIS_DAP_"
# — its SWD interface is not a tty, so only the UART bridge shows up). The USB-C debug port does NOT
# enumerate a serial device on this board, so the header + probe is the console.
#
# This is the exact sibling of pi-serial-bridge.py; the same two macOS traps make a naive
# `stty` + `cat` fail, so one tool owns the port:
#   1. ONE read-write fd, opened ONCE. A separate `stty 115200 ...` then a fresh `cat < dev` re-opens
#      the CDC port, resetting its baud to 9600 -> the console reads as garbage (all-high-bit bytes).
#      So we open the device a single time O_RDWR and set termios on that same fd.
#   2. A held-open O_RDWR command FIFO. A shell FIFO pump dies under backgrounding, and an O_RDONLY
#      FIFO reports readable-with-0-bytes forever once the first writer closes. O_RDWR keeps it alive
#      across many `printf ... > fifo` injections.
#
# One select() loop: serial -> (log file + stdout), FIFO -> serial. Capture is 115200 8N1 raw, no
# echo. Run it as a background process; inject console input with `printf 'CMD\r' > ~/unaos-bench/scratch/jetson.in`.
#
# Driving the JetPack UEFI boot (send every line with a trailing CR, i.e. \r):
#   - tap ESC during firmware boot if it does not drop to the UEFI Shell (Shell>).
#   - connect -r                        bind FAT drivers (card readers / USB need this)
#   - map -r                            list filesystems; find the stick's FSx: (HD(1,MBR,...))
#   - FSx:\EFI\BOOT\BOOTAA64.EFI        launch UnaOS (x = the number `map -r` showed)
# After launch, expect (with UNAOS_BOOTDIAG=1): the `BOOTDIAG:` block, the bootloader proceeding past
# the old GOP stop, then the kernel banner + `:: tegra: early platform stop ... ::` + `:: tegra:
# heartbeat <n> ::`. If the kernel goes silent after ExitBootServices, the tegra UARTC base is the
# prime suspect (the BOOTDIAG /chosen stdout-path names the real console UART) — STOP and report.
#
# To STOP: kill by DEVICE, never `pkill -f cat`/`-f python` (that has killed unrelated apps and the
# agent itself):  for p in $(lsof -t /dev/cu.usbmodem*); do kill "$p"; done
#
# Usage:  jetson-serial-bridge.py [DEV] [LOG] [FIFO]
#   DEV   serial device       (default: auto-detect the first /dev/cu.usbmodem*)
#   LOG   capture file         (default: ./jetson-serial.log, appended)
#   FIFO  command inject FIFO   (default: ~/unaos-bench/scratch/jetson.in)
import os, sys, glob, select, termios, time, errno


def find_dev():
    devs = sorted(glob.glob("/dev/cu.usbmodem*"))
    return devs[0] if devs else None


def open_serial(dev):
    """Open `dev` O_RDWR and put it in raw 115200 8N1, no echo — mirrors
    `stty 115200 cs8 -cstopb -parenb -echo raw` but on a single, never-reopened fd."""
    fd = os.open(dev, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
    attrs = termios.tcgetattr(fd)
    iflag, oflag, cflag, lflag, ispeed, ospeed, cc = attrs
    iflag = 0                                             # no input processing (raw, no CR/NL xlate)
    oflag = 0                                             # no output processing
    lflag = 0                                             # no canonical mode, no echo, no signals
    cflag = termios.CS8 | termios.CREAD | termios.CLOCAL  # 8N1, receiver on, ignore modem lines
    ispeed = ospeed = termios.B115200
    cc = list(cc)
    cc[termios.VMIN] = 0
    cc[termios.VTIME] = 0
    termios.tcsetattr(fd, termios.TCSANOW, [iflag, oflag, cflag, lflag, ispeed, ospeed, cc])
    return fd


USAGE = """jetson-serial-bridge.py — bidirectional Jetson Orin Debug Probe serial bridge (macOS).

Owns a CDC-ACM probe on a single O_RDWR fd (115200 8N1 raw) + a held-open command FIFO, in one
select() loop: serial -> log + stdout, FIFO -> serial. See the header comment for the macOS gotchas
and the UEFI-Shell boot-drive recipe.

  Usage:  jetson-serial-bridge.py [DEV] [LOG] [FIFO]
    DEV   serial device       (default: auto-detect the first /dev/cu.usbmodem*)
    LOG   capture file         (default: ./jetson-serial.log, appended)
    FIFO  command inject FIFO   (default: ~/unaos-bench/scratch/jetson.in)

  Inject console input (trailing CR!):  printf 'connect -r\\r' > ~/unaos-bench/scratch/jetson.in
  Stop by DEVICE (never `pkill -f cat`):  for p in $(lsof -t /dev/cu.usbmodem*); do kill "$p"; done"""


def main(argv):
    if any(a in ("-h", "--help") for a in argv[1:]):
        print(USAGE)
        return 0

    dev = argv[1] if len(argv) > 1 else find_dev()
    log = argv[2] if len(argv) > 2 else os.path.join(os.getcwd(), "jetson-serial.log")
    fifo = os.path.expanduser(argv[3] if len(argv) > 3 else "~/unaos-bench/scratch/jetson.in")

    if not dev:
        print("BRIDGE: no /dev/cu.usbmodem* found — is the Debug Probe plugged in?", flush=True)
        return 1

    if not os.path.exists(fifo):
        # ~/unaos-bench/scratch/ is not guaranteed to exist the way /tmp was (Peter, 2026-09-02:
        # nothing under /tmp — it is RAM-backed and swept). Make the parent, then the FIFO.
        os.makedirs(os.path.dirname(fifo) or ".", exist_ok=True)
        os.mkfifo(fifo)
    fifo_fd = os.open(fifo, os.O_RDWR | os.O_NONBLOCK)

    logf = open(log, "ab", buffering=0)
    ser = open_serial(dev)
    print(f"BRIDGE: {dev} @115200  log={log}  fifo={fifo}  "
          f"(inject:  printf 'connect -r\\r' > {fifo} )", flush=True)

    while True:
        try:
            r, _, _ = select.select([ser, fifo_fd], [], [], 1.0)
            if ser in r:
                try:
                    data = os.read(ser, 4096)
                except OSError as e:
                    if e.errno in (errno.EAGAIN, errno.EWOULDBLOCK):
                        data = b""
                    else:
                        raise
                if data:
                    logf.write(data)
                    sys.stdout.buffer.write(data)   # mirror so a tail/Monitor can react live
                    sys.stdout.flush()
            if fifo_fd in r:
                try:
                    cmd = os.read(fifo_fd, 4096)
                except OSError as e:
                    cmd = b"" if e.errno in (errno.EAGAIN, errno.EWOULDBLOCK) else b""
                if cmd:
                    os.write(ser, cmd)
        except OSError:
            # The probe re-enumerated (Orin power-cycle / replug), possibly under a new node number.
            try:
                os.close(ser)
            except OSError:
                pass
            time.sleep(0.5)
            newdev = dev if os.path.exists(dev) else find_dev()
            if newdev:
                try:
                    ser = open_serial(newdev)
                    print(f"BRIDGE: reopened {newdev}", flush=True)
                except OSError:
                    time.sleep(0.5)
            else:
                time.sleep(0.5)


if __name__ == "__main__":
    sys.exit(main(sys.argv))
