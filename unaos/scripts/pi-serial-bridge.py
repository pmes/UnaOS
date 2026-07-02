#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 The Architect & Una
#
# pi-serial-bridge.py — bidirectional serial bridge for the Raspberry Pi 4 Debug Probe (macOS).
#
# The bare-metal Pi 4 boot is verified over the official Debug Probe (a CDC-ACM USB serial device,
# /dev/cu.usbmodem*). Two macOS-specific traps make the naive `stty` + `cat` approach fail, so this
# single tool owns the port instead:
#
#   1. ONE read-write fd, opened ONCE. A separate `stty 115200 ...` followed by a fresh `cat < dev`
#      re-opens the CDC port, which resets its baud to 9600 -> the handoff reads as garbage. So we
#      open the device a single time with O_RDWR and configure termios on that same fd.
#   2. A held-open O_RDWR command FIFO. A shell FIFO pump (`cat fifo > dev`) dies under backgrounding,
#      and a plain O_RDONLY FIFO reports readable-with-0-bytes forever once the first writer closes.
#      Opening the FIFO O_RDWR keeps it alive across many `printf ... > fifo` injections.
#
# It runs a single select() loop: serial -> (log file + stdout), FIFO -> serial. Capture is at
# 115200 8N1 raw, no echo. Run it as a background process; inject typed input with, e.g.,
# `printf 'panic\r' > /tmp/pi.in`. Once the GUI owns the HDMI screen, typed input renders to the
# panel and is NOT echoed to serial, so use a serial-observable command (`panic`) for a round-trip
# check, or eyeball the HDMI.
#
# To STOP readers: kill by DEVICE, never `pkill -f cat` (that matches "/AppliCATions/" and has killed
# Chrome and the agent itself, twice):  for p in $(lsof -t /dev/cu.usbmodem*); do kill "$p"; done
#
# Usage:  pi-serial-bridge.py [DEV] [LOG] [FIFO]
#   DEV   serial device       (default: auto-detect the first /dev/cu.usbmodem*)
#   LOG   capture file         (default: ./pi-serial.log, appended)
#   FIFO  command inject FIFO   (default: /tmp/pi.in)
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


USAGE = """pi-serial-bridge.py — bidirectional Raspberry Pi 4 Debug Probe serial bridge (macOS).

Owns a CDC-ACM probe on a single O_RDWR fd (115200 8N1 raw) + a held-open command FIFO, in one
select() loop: serial -> log + stdout, FIFO -> serial. See the header comment for the macOS gotchas.

  Usage:  pi-serial-bridge.py [DEV] [LOG] [FIFO]
    DEV   serial device       (default: auto-detect the first /dev/cu.usbmodem*)
    LOG   capture file         (default: ./pi-serial.log, appended)
    FIFO  command inject FIFO   (default: /tmp/pi.in)

  Inject typed input:  printf 'panic\\r' > /tmp/pi.in
  Stop by DEVICE (never `pkill -f cat`):  for p in $(lsof -t /dev/cu.usbmodem*); do kill "$p"; done"""


def main(argv):
    if any(a in ("-h", "--help") for a in argv[1:]):
        print(USAGE)
        return 0

    dev = argv[1] if len(argv) > 1 else find_dev()
    log = argv[2] if len(argv) > 2 else os.path.join(os.getcwd(), "pi-serial.log")
    fifo = argv[3] if len(argv) > 3 else "/tmp/pi.in"

    if not dev:
        print("BRIDGE: no /dev/cu.usbmodem* found — is the Debug Probe plugged in?", flush=True)
        return 1

    if not os.path.exists(fifo):
        os.mkfifo(fifo)
    fifo_fd = os.open(fifo, os.O_RDWR | os.O_NONBLOCK)

    logf = open(log, "ab", buffering=0)
    ser = open_serial(dev)
    print(f"BRIDGE: {dev} @115200  log={log}  fifo={fifo}  "
          f"(inject:  printf 'ver\\r' > {fifo} )", flush=True)

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
            # The probe re-enumerated (Pi power-cycle / replug), possibly under a new node number.
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
