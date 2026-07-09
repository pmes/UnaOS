#!/bin/bash
# jetson-bench-connect.sh — one-command host-side bench connect for the Jetson Orin serial console
# (the "boot-log action"). The jetson sibling of rmbp-bench-connect.sh.
#
# Reliably (re)establishes the boot-log capture no matter what state the last session left things in:
#   1. finds the RPi Debug Probe on the board's TTL UART header (first /dev/cu.usbmodem* — the USB-C
#      port never enumerates a console on this board),
#   2. kills any process holding it — BY DEVICE via lsof, never `pkill -f` (see unaos-hazards),
#   3. starts jetson-serial-bridge.py against a fresh dated log in ~/unaos-bench/ (outside target/)
#      with a held-open command FIFO (/tmp/jetson.in),
#   4. re-points ~/jetson-serial.log at that fresh log — the STABLE path to tail (this REFRESH is
#      what makes the symlink trustworthy; a stale ~/jetson-serial.log from an old session is the
#      documented trap).
#
# Usage:  scripts/jetson-bench-connect.sh [DEV] [FIFO]
#   DEV    serial device  (default: auto-detect first /dev/cu.usbmodem*)
#   FIFO   inject FIFO     (default: /tmp/jetson.in)
# Foreground by design — run behind the session's run_in_background, or with `&`/tmux from a shell.
# Drive the UEFI Shell by injecting lines WITH a trailing CR, e.g.:
#     printf 'connect -r\r' > /tmp/jetson.in ; printf 'map -r\r' > /tmp/jetson.in
#     printf 'FS0:\EFI\BOOT\BOOTAA64.EFI\r' > /tmp/jetson.in
# If the bridge process dies, just run this script again — that IS the recovery procedure.
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
BENCH_DIR="${HOME}/unaos-bench"
LINK="${HOME}/jetson-serial.log"
FIFO="${2:-/tmp/jetson.in}"

# 1. Device: argument wins, else auto-detect the RPi Debug Probe (a CDC-ACM /dev/cu.usbmodem*).
#    The device path drifts across probe re-enumerations, so always re-detect (never hardcode).
DEV="${1:-}"
if [ -z "$DEV" ]; then
    for d in /dev/cu.usbmodem*; do
        [ -e "$d" ] && DEV="$d" && break
    done
fi
if [ -z "$DEV" ] || [ ! -e "$DEV" ]; then
    echo "jetson-bench-connect: no RPi Debug Probe found (/dev/cu.usbmodem*)." >&2
    echo "  Is the probe on the board TTL header (pin3=RX pin4=TX) and plugged into THIS host?" >&2
    echo "  ('ls /dev/cu.usbmodem*' to inspect — the path changes on each replug.)" >&2
    exit 1
fi

# 2. Free the device — kill holders by device (never pkill -f), then give them a beat to exit.
HOLDERS="$(lsof -t "$DEV" 2>/dev/null || true)"
if [ -n "$HOLDERS" ]; then
    echo "jetson-bench-connect: killing current holder(s) of $DEV: $HOLDERS"
    for p in $HOLDERS; do kill "$p" 2>/dev/null || true; done
    sleep 1
fi

# 3. Fresh dated log outside target/.
mkdir -p "$BENCH_DIR"
LOG="${BENCH_DIR}/jetson-serial-$(date +%Y-%m-%d-%H%M%S).log"
touch "$LOG"

# 4. Refresh the stable watch path.
ln -sf "$LOG" "$LINK"
echo "jetson-bench-connect: device=$DEV"
echo "jetson-bench-connect: log=$LOG"
echo "jetson-bench-connect: fifo=$FIFO  (inject:  printf 'connect -r\\r' > $FIFO )"
echo "jetson-bench-connect: watch it at $LINK  (e.g. 'tail -f ~/jetson-serial.log' — read with awk/grep -a)"

exec python3 "${HERE}/jetson-serial-bridge.py" "$DEV" "$LOG" "$FIFO"
