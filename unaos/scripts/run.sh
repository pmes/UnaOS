#!/bin/bash
set -e

# unaOS Cross-Platform UEFI Runner
# Handles ESP creation and QEMU invocation for Linux and macOS.

EFI_EXE="$1"
ESP_DIR="target/esp"
OVMF_PATH=""

# 1. Firmware Discovery
# List of potential paths for OVMF firmware (Code)
# We prioritize separate CODE/VARS split, but here we just look for the main executable firmware image.
POTENTIAL_PATHS=(
    "/usr/share/ovmf/OVMF.fd"                          # Debian/Ubuntu (Standard)
    "/usr/share/edk2/ovmf/OVMF_CODE.fd"                # Fedora
    "/usr/share/OVMF/OVMF_CODE.fd"                     # Fedora (Alt)
    "/usr/local/share/qemu/edk2-x86_64-code.fd"        # macOS (Homebrew Intel)
    "/opt/homebrew/share/qemu/edk2-x86_64-code.fd"     # macOS (Homebrew Silicon)
    "./OVMF.fd"                                        # Local fallback
)

echo "=== unaOS UEFI Runner ==="
echo "Host OS: $(uname -s)"

for path in "${POTENTIAL_PATHS[@]}"; do
    if [ -f "$path" ]; then
        OVMF_PATH="$path"
        echo "Firmware Found: $OVMF_PATH"
        break
    fi
done

if [ -z "$OVMF_PATH" ]; then
    echo "CRITICAL ERROR: OVMF Firmware not found."
    echo "Checked the following locations:"
    printf "  - %s\n" "${POTENTIAL_PATHS[@]}"
    echo "Please install 'ovmf' (Linux) or 'qemu' (macOS) to proceed."
    exit 1
fi

# 2. ESP Creation
echo "Creating ESP at $ESP_DIR..."
mkdir -p "$ESP_DIR/EFI/BOOT"

# Copy and Rename to BOOTX64.EFI (The default fallback bootloader path)
cp "$EFI_EXE" "$ESP_DIR/EFI/BOOT/BOOTX64.EFI"
echo "Bootloader installed to: $ESP_DIR/EFI/BOOT/BOOTX64.EFI"

# 3. Execution
echo "Launching QEMU..."
# Note: We use -display none because we are likely in a headless CI or remote environment,
# relying on -serial stdio for output. If a window is needed, remove -display none.
# However, the previous successful run used -display none to avoid GTK errors.
# We will use -display none based on the environment observations.

qemu-system-x86_64 \
    -drive if=pflash,format=raw,readonly=on,file="$OVMF_PATH" \
    -drive format=raw,file=fat:rw:"$ESP_DIR" \
    -serial stdio \
    -no-reboot \
    -no-shutdown \
    -d guest_errors
