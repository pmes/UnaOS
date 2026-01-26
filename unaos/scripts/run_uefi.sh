#!/bin/bash
# $1 is the binary passed by cargo

BIOS_SRC="/usr/local/share/qemu/edk2-x86_64-code.fd"
LOCAL_DIR="target"
BIOS_LOCAL="${LOCAL_DIR}/OVMF.fd"
EFI_EXE="$1"
PWD_PATH=$(pwd)
ABS_BIOS_LOCAL="${PWD_PATH}/${BIOS_LOCAL}"

# Ensure local copy exists
mkdir -p "$LOCAL_DIR"
if [ -f "$BIOS_SRC" ]; then
    cp "$BIOS_SRC" "$BIOS_LOCAL"
    cp "$BIOS_SRC" ./OVMF.fd
else
    echo "WARNING: Source BIOS not found at $BIOS_SRC"
fi

echo "=== Starting UEFI Runner ==="
echo "Target: $EFI_EXE"

try_run() {
    DESC=$1
    CMD=$2
    echo "--------------------------------------------------"
    echo "Attempting: $DESC"
    echo "Command: $CMD"
    eval "$CMD"
    RET=$?
    if [ $RET -eq 0 ]; then
        echo "SUCCESS: Strategy '$DESC' worked."
        exit 0
    else
        echo "FAILED: Strategy '$DESC' returned $RET"
    fi
}

# Strategy 0: Direct QEMU Invocation (Bypass uefi-run entirely)
# This creates a manual ESP structure and uses -drive for pflash to avoid -bios limitations
ESP_DIR="${LOCAL_DIR}/esp"
mkdir -p "${ESP_DIR}/EFI/BOOT"
cp "$EFI_EXE" "${ESP_DIR}/EFI/BOOT/BOOTX64.EFI"

# Note: We use format=raw because we just copied the fd file.
# We mount the ESP directory as a FAT drive.
try_run "Direct QEMU (pflash + fat:rw)" "qemu-system-x86_64 \
    -drive if=pflash,format=raw,readonly=on,file=\"$ABS_BIOS_LOCAL\" \
    -drive format=raw,file=fat:rw:\"$ESP_DIR\" \
    -serial stdio \
    -no-reboot -no-shutdown \
    -d guest_errors"

# Strategy 1: Absolute Local Path (Bypasses relative path sandbox issues)
try_run "Absolute Local Path" "uefi-run -b \"$ABS_BIOS_LOCAL\" -q qemu-system-x86_64 \"$EFI_EXE\" -- -d guest_errors -serial stdio"

# Strategy 2: Original System Path
try_run "System Path" "uefi-run -b \"$BIOS_SRC\" -q qemu-system-x86_64 \"$EFI_EXE\" -- -d guest_errors -serial stdio"

# Strategy 3: Default (CWD) - relies on ./OVMF.fd existing
try_run "CWD Default" "uefi-run -q qemu-system-x86_64 \"$EFI_EXE\" -- -d guest_errors -serial stdio"

# Strategy 4: Relative Local Path (What failed previously, but worth keeping as fallback)
try_run "Relative Local Path" "uefi-run -b \"$BIOS_LOCAL\" -q qemu-system-x86_64 \"$EFI_EXE\" -- -d guest_errors -serial stdio"

echo "--------------------------------------------------"
echo "FATAL: All strategies failed to launch QEMU."
exit 1
