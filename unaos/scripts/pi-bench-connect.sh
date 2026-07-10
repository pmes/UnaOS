#!/bin/bash
# pi-bench-connect.sh — one-command host-side bench connect for the Pi 4 Debug Probe serial console.
# The pi4 sibling of rmbp-bench-connect.sh / jetson-bench-connect.sh.
#
# Reliably (re)establishes the boot-log capture no matter what state the last session left things in:
#   1. finds the RPi Debug Probe (first /dev/cu.usbmodem* — the path drifts on every replug),
#   2. kills any process holding it — BY DEVICE via lsof, never `pkill -f` (see unaos-hazards),
#   3. starts pi-serial-bridge.py against a fresh dated log in ~/unaos-bench/ (outside target/)
#      with a held-open command FIFO (/tmp/pi.in),
#   4. re-points ~/pi-serial.log at that fresh log — the STABLE path to tail (this REFRESH is what
#      makes the symlink trustworthy; a stale link from an old session is the documented trap).
#
# Usage:  scripts/pi-bench-connect.sh [DEV] [FIFO]
#   DEV    serial device  (default: auto-detect first /dev/cu.usbmodem*)
#   FIFO   inject FIFO     (default: /tmp/pi.in)
# Foreground by design — run behind the session's run_in_background, or with `&`/tmux from a shell.
# Inject typed input with a trailing CR, e.g.:  printf 'panic\r' > /tmp/pi.in
# (once the GUI owns the HDMI screen, typed input does NOT echo to serial — `panic` is the
# serial-observable round-trip check; it halts).
# If the bridge process dies, just run this script again — that IS the recovery procedure.
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
BENCH_DIR="${HOME}/unaos-bench"
LINK="${HOME}/pi-serial.log"
FIFO="${2:-/tmp/pi.in}"

# 1. Device: argument wins, else auto-detect the RPi Debug Probe (a CDC-ACM /dev/cu.usbmodem*).
DEV="${1:-}"
if [ -z "$DEV" ]; then
    for d in /dev/cu.usbmodem*; do
        [ -e "$d" ] && DEV="$d" && break
    done
fi
if [ -z "$DEV" ] || [ ! -e "$DEV" ]; then
    echo "pi-bench-connect: no RPi Debug Probe found (/dev/cu.usbmodem*)." >&2
    echo "  Is the probe on the Pi's UART header and plugged into THIS host?" >&2
    echo "  ('ls /dev/cu.usbmodem*' to inspect — the path changes on each replug.)" >&2
    exit 1
fi

# 2. Free the device — kill holders by device (never pkill -f), then give them a beat to exit.
HOLDERS="$(lsof -t "$DEV" 2>/dev/null || true)"
if [ -n "$HOLDERS" ]; then
    echo "pi-bench-connect: killing current holder(s) of $DEV: $HOLDERS"
    for p in $HOLDERS; do kill "$p" 2>/dev/null || true; done
    sleep 1
fi

# 3. Fresh dated log outside target/.
mkdir -p "$BENCH_DIR"
LOG="${BENCH_DIR}/pi-serial-$(date +%Y-%m-%d-%H%M%S).log"
touch "$LOG"

# 4. Refresh the stable watch path.
ln -sf "$LOG" "$LINK"
echo "pi-bench-connect: device=$DEV"
echo "pi-bench-connect: log=$LOG"
echo "pi-bench-connect: fifo=$FIFO  (inject:  printf 'panic\\r' > $FIFO )"
echo "pi-bench-connect: watch it at $LINK  (e.g. 'tail -f ~/pi-serial.log' — read with awk/grep -a)"

exec python3 "${HERE}/pi-serial-bridge.py" "$DEV" "$LOG" "$FIFO"
