#!/bin/bash
# QEMUIMAGE26 launcher: the bench's exact line, + the QEMU door plumbing (USBSERIAL, FTDIRX_INJECT, QMP, KVM).
set -u
W=/home/pmes/src/github.com/pmes/UnaOS-hw-pi4/.claude/worktrees/agent-a6af1cf8df5c43780
S=/tmp/claude-1000/-home-pmes-src-github-com-pmes-UnaOS/d109204f-6aa4-4cba-b358-efd3b8453b2d/scratchpad/q26
for v in $(compgen -e | awk '/^UNAOS_/'); do unset "$v"; done
cd "$W/unaos" || exit 1
# --- the bench's line, verbatim ---
export UNAOS_WIFI_FW_PATH=/home/pmes/src/github.com/pmes/UnaOS-bunker/drivers/broadcom-b43; set -a; . ./scripts/image-line.env; set +a; export UNAOS_AETHER=1 UNAOS_MIDDEN=1 UNAOS_UNA=1 UNAOS_SONDE=1
# --- QEMU-only plumbing (builder knobs, not kernel features) ---
SOCK="$W/unaos/target/f.sock"; PORT=4490   # a UNIX socket path must be < 108 bytes (the scratchpad path is not)
while lsof -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1; do PORT=$((PORT+1)); done
export UNAOS_USBSERIAL=1 UNAOS_FTDIRX_INJECT="$SOCK" UNAOS_QEMU_FULL=1
export UNAOS_QEMU_EXTRA="-accel kvm -qmp tcp:127.0.0.1:${PORT},server,nowait"
compgen -e | awk '/^UNAOS_/' | sort > "$S/knobs.txt"
echo "knobs=$(wc -l < "$S/knobs.txt") port=$PORT sock=$SOCK"
rm -f target/sdcard.img target/serial.log
python3 "$S/drive.py" "$W/unaos" "$SOCK" "$PORT" "$W/unaos/target/serial.log" "$S" 1500 > "$S/drive.log" 2>&1 &
echo "drive_pid=$!" > "$S/pids"
date +%s > "$S/t_start"
./arroyo twoboot --boot 1 1560 > "$S/arroyo.out" 2>&1 &
echo "arroyo_pid=$!" >> "$S/pids"
wait
