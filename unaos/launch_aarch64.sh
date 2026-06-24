#!/bin/bash
set -e

WORKSPACE_DIR=$(pwd)
ESP_DIR="${WORKSPACE_DIR}/target/aarch64_esp"
KERNEL_BIN="${WORKSPACE_DIR}/target/aarch64-unaos/release/unaos-kernel"
BOOTLOADER_BIN="${WORKSPACE_DIR}/target/aarch64-unknown-uefi/release/bootloader.efi"

echo "🔹 Building aarch64 Kernel..."
cd crates/kernel
cargo +nightly build --release --target ../../aarch64-unaos.json -Z build-std=core,compiler_builtins,alloc -Z build-std-features=compiler-builtins-mem -Z json-target-spec
cd ../..

echo "🔹 Building aarch64 UEFI Bootloader..."
cd crates/bootloader
cargo +nightly build --release --target aarch64-unknown-uefi -Z build-std=core,compiler_builtins,alloc -Z build-std-features=compiler-builtins-mem
cd ../..

echo "🔹 Packaging ESP (EFI System Partition)..."
rm -rf "$ESP_DIR"
mkdir -p "$ESP_DIR/EFI/BOOT"
cp "$BOOTLOADER_BIN" "$ESP_DIR/EFI/BOOT/BOOTAA64.EFI"
cp "$KERNEL_BIN" "$ESP_DIR/kernel.elf"

echo "🔹 Locating AAVMF (AArch64 UEFI Firmware)..."
AAVMF_PATHS=(
    "/usr/share/AAVMF/AAVMF_CODE.fd"
    "/usr/share/edk2/aarch64/QEMU_EFI.fd"
    "/usr/share/qemu-efi-aarch64/QEMU_EFI.fd"
    "/usr/local/share/qemu/edk2-aarch64-code.fd"
    "/opt/homebrew/share/qemu/edk2-aarch64-code.fd"
)

AAVMF_PATH=""
for path in "${AAVMF_PATHS[@]}"; do
    if [ -f "$path" ]; then
        AAVMF_PATH="$path"
        break
    fi
done

if [ -z "$AAVMF_PATH" ]; then
    echo "CRITICAL ERROR: AAVMF Firmware not found."
    exit 1
fi

echo "🔹 Launching QEMU..."
qemu-system-aarch64 \
    -machine virt,virtualization=on \
    -cpu cortex-a72 \
    -smp 4 \
    -m 1G \
    -drive if=pflash,format=raw,readonly=on,file="$AAVMF_PATH" \
    -drive format=raw,file=fat:rw:"$ESP_DIR" \
    -serial stdio \
    -device ramfb \
    -no-reboot \
    -no-shutdown \
    -d guest_errors
