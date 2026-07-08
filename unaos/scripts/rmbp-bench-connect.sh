#!/bin/bash
# rmbp-bench-connect.sh — one-command host-side bench connect for the rMBP FTDI serial console.
#
# Reliably (re)establishes the capture no matter what state the last session left things in:
#   1. finds the FTDI device (first /dev/cu.usbserial*, else /dev/cu.usbmodem*),
#   2. kills any process holding it — BY DEVICE via lsof, never `pkill -f` (see unaos-hazards),
#   3. starts x86-serial-bridge.py against a fresh dated log in ~/unaos-bench/ (outside target/),
#   4. symlinks ~/rmbp-serial.log to that log — the STABLE path both Peter and the session watch.
#
# Usage:  scripts/rmbp-bench-connect.sh [DEV]
#   DEV   serial device (default: auto-detect)
# Foreground by design — run it in the background from the session (run_in_background) or with
# `&`/tmux from a shell. Survives an FTDI replug (the bridge reopens on ENXIO); if the bridge
# process itself dies, just run this script again — that IS the recovery procedure.
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
BENCH_DIR="${HOME}/unaos-bench"
LINK="${HOME}/rmbp-serial.log"

# 1. Device: argument wins, else auto-detect (same order as the bridge's own detection).
DEV="${1:-}"
if [ -z "$DEV" ]; then
    for d in /dev/cu.usbserial* /dev/cu.usbmodem*; do
        [ -e "$d" ] && DEV="$d" && break
    done
fi
if [ -z "$DEV" ] || [ ! -e "$DEV" ]; then
    echo "rmbp-bench-connect: no FTDI device found (/dev/cu.usbserial* or /dev/cu.usbmodem*)." >&2
    echo "  Is the cable plugged into THIS host? ('ls /dev/cu.*' to inspect.)" >&2
    exit 1
fi

# 2. Free the device — kill holders by device (never pkill -f), then give them a beat to exit.
HOLDERS="$(lsof -t "$DEV" 2>/dev/null || true)"
if [ -n "$HOLDERS" ]; then
    echo "rmbp-bench-connect: killing current holder(s) of $DEV: $HOLDERS"
    for p in $HOLDERS; do kill "$p" 2>/dev/null || true; done
    sleep 1
fi

# 3. Fresh dated log outside target/.
mkdir -p "$BENCH_DIR"
LOG="${BENCH_DIR}/rmbp-serial-$(date +%Y-%m-%d-%H%M%S).log"
touch "$LOG"

# 4. Stable watch path.
ln -sf "$LOG" "$LINK"
echo "rmbp-bench-connect: device=$DEV"
echo "rmbp-bench-connect: log=$LOG"
echo "rmbp-bench-connect: watch it at $LINK  (e.g. 'tail -f ~/rmbp-serial.log' — read with awk/grep -a)"

exec python3 "${HERE}/x86-serial-bridge.py" "$DEV" "$LOG"
