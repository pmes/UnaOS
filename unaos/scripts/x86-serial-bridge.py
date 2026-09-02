#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 The Architect & Una
#
# x86-serial-bridge.py — host-side serial capture for the 2012 rMBP (x86_64) FTDI USB-serial console (macOS).
#
# The 2012 rMBP (MacBookPro10,1) has NO 16550 UART, so its metal boot is observed over a USB-serial cable driven
# by the kernel's OWN xHCI stack (`drivers/xhci/ftdi.rs` + `arch/x86_64/serial.rs`): every `serial_print!` mirrors
# into an in-kernel boot-capture ring from the first print; when the FT232 (VID 0x0403 / PID 0x6001) enumerates on
# the rMBP's xHCI bus and `service_ftdi` brings it up (RESET -> SET_BAUDRATE 115200 -> SET_DATA 8N1 ->
# SET_FLOW_CTRL), the ENTIRE early boot log replays out the cable, then live output drains. Capture is 115200 8N1.
#
# IMPORTANT — the kernel's FTDI console is TX-ONLY this arc. FTDI bulk-IN (RX, i.e. host->target typing) is a STUB:
# the kernel does not service it, so ANY bytes this bridge injects toward the target are ignored by the kernel.
# You READ the replay; you cannot type over it on metal yet. The inject FIFO below is retained for symmetry with
# the pi/jetson bridges and for the eventual RX arc, but today it is a deliberate no-op against this kernel.
#
# This tool owns the host serial port on a single, never-reopened O_RDWR fd, mirroring the two macOS gotchas the
# pi bridge documents:
#   1. ONE read fd, opened ONCE. A separate `stty ...` then a fresh `cat < dev` re-opens the port and can reset its
#      baud -> the handoff reads as garbage. So we open the device once, O_RDWR, and set termios on that same fd.
#   2. A held-open O_RDWR command FIFO survives shell backgrounding + many `printf ... > fifo` injections (a plain
#      O_RDONLY FIFO reports readable-with-0-bytes forever once the first writer closes).
#
# It runs a single select() loop: serial -> (log file + stdout), FIFO -> serial (no-op vs the TX-only kernel).
# Capture is 115200 8N1 raw, no echo. Run it as a background process (`run_in_background`), read the log by DEVICE.
#
# The host serial NODE depends on how the target's TX reaches this Mac (confirm the wiring at the bench). An FTDI
# FT232 on macOS's VCP driver enumerates as /dev/cu.usbserial-*; some adapters show as /dev/cu.usbmodem*. This
# tool auto-detects usbserial first, then usbmodem.
#
# To STOP readers: kill by DEVICE, never `pkill -f cat`/`pkill -f python` (that has killed Chrome and the agent
# itself before):  for p in $(lsof -t /dev/cu.usbserial* /dev/cu.usbmodem*); do kill "$p"; done
#
# Usage:  x86-serial-bridge.py [DEV] [LOG] [FIFO]
#   DEV   serial device       (default: auto-detect first /dev/cu.usbserial* then /dev/cu.usbmodem*)
#   LOG   capture file         (default: ./x86-serial.log, appended)
#   FIFO  command inject FIFO   (default: ~/unaos-bench/scratch/x86.in ; a no-op against the TX-only kernel)
import os, sys, glob, select, termios, time, errno


def find_dev():
    # FTDI FT232 on macOS's VCP driver is /dev/cu.usbserial-* ; fall back to /dev/cu.usbmodem* (some adapters).
    devs = sorted(glob.glob("/dev/cu.usbserial*")) or sorted(glob.glob("/dev/cu.usbmodem*"))
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


USAGE = """x86-serial-bridge.py — host-side capture for the rMBP FTDI USB-serial console (macOS).

Owns a host serial port on a single O_RDWR fd (115200 8N1 raw) + a held-open command FIFO, in one select()
loop: serial -> log + stdout, FIFO -> serial. The kernel's FTDI console is TX-ONLY this arc, so inject is a
no-op against it (kept for symmetry + the future RX arc). See the header comment for the macOS gotchas.

  Usage:  x86-serial-bridge.py [DEV] [LOG] [FIFO]
    DEV   serial device       (default: auto-detect first /dev/cu.usbserial* then /dev/cu.usbmodem*)
    LOG   capture file         (default: ./x86-serial.log, appended)
    FIFO  command inject FIFO   (default: ~/unaos-bench/scratch/x86.in ; a no-op against the TX-only kernel)

  Stop by DEVICE (never `pkill -f`):
    for p in $(lsof -t /dev/cu.usbserial* /dev/cu.usbmodem*); do kill "$p"; done"""


def main(argv):
    if any(a in ("-h", "--help") for a in argv[1:]):
        print(USAGE)
        return 0

    dev = argv[1] if len(argv) > 1 else find_dev()
    log = argv[2] if len(argv) > 2 else os.path.join(os.getcwd(), "x86-serial.log")
    fifo = argv[3] if len(argv) > 3 else os.path.expanduser("~/unaos-bench/scratch/x86.in")  # never /tmp (bench law: RAM-backed, 3-day cleared)

    if not dev:
        print("BRIDGE: no /dev/cu.usbserial* or /dev/cu.usbmodem* found — is the FTDI cable plugged in?",
              flush=True)
        return 1

    # /tmp always existed; scratch may not — a fresh host must not fail at its first mkfifo.
    os.makedirs(os.path.dirname(fifo), exist_ok=True)
    if not os.path.exists(fifo):
        os.mkfifo(fifo)
    fifo_fd = os.open(fifo, os.O_RDWR | os.O_NONBLOCK)

    logf = open(log, "ab", buffering=0)
    ser = open_serial(dev)
    print(f"BRIDGE: {dev} @115200 8N1  log={log}  fifo={fifo}  (TX-only kernel: inject is a no-op)", flush=True)

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
                    os.write(ser, cmd)              # reaches the target's RX pin; the TX-only kernel ignores it
        except OSError:
            # The FTDI re-enumerated (rMBP power-cycle / replug), possibly under a new node number.
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
