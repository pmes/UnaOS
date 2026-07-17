# orin-xhci-repro

A standalone UEFI reproducer for the Tegra234 / Jetson Orin xHCI-takeover RAS carveout
fault reported to NVIDIA on their developer forum, topic 377113:
<https://forums.developer.nvidia.com/t/orin-nano-r39-2-ras-carveout-fault-on-crcr-write-to-uefi-inherited-xhci-identical-binary-faults-one-day-boots-clean-the-next/377113>

This is defensive/diagnostic tooling built at NVIDIA's own request (they asked, on that
forum topic, for a reliable way to reproduce the fault on their bench). There is no exploit
here and no target other than the operator's own Jetson Orin board.

## What it is

A minimal `#![no_std]` UEFI application, built with the [`uefi`](https://crates.io/crates/uefi)
crate (the same crate and version the UnaOS bootloader already uses). It needs **no UnaOS
kernel, no UnaOS build tooling, and no `arroyo`** — just a UEFI boot stick carrying the
built `.efi` binary.

It runs entirely read-only except for one explicitly gated register write:

1. Locates the Tegra234 XUSB/xHCI host controller at its fixed platform MMIO base
   (`0x0361_0000` — the `usb@3610000` device-tree node; the same address UnaOS's own
   kernel takeover and NVIDIA's own `XhciControllerDxe` use — this is a well-known fixed
   SoC address, not something probed for dynamically).
2. Reads and prints USBCMD, USBSTS, CONFIG, and CRCR, and confirms USBCMD.RS=1 (the
   controller is UEFI-inherited and running — the fault's stated precondition).
3. Prints a loud multi-line warning banner and waits for an explicit `Y` keypress.
4. On confirmation, issues exactly one 64-bit write to CRCR: it reads the register and
   writes the *same* value straight back (content-free — the pointer field of a CRCR read
   is always zero per xHCI 5.4.5, so this changes no controller state). On an
   affected/"poisoned" boot this alone is expected to trigger an immediate RAS power-off.

See the doc comments at the top of `src/main.rs` for the full provenance of the register
sequence — it is traced directly from `unaos/crates/kernel/src/arch/aarch64/xusb_tegra.rs`
(the `jbxc_crcrq_quiesce` function and its call site in `jb2b_attach`), which is where
UnaOS's own bench work (the 2026-07-16 "CRCR+SMP-7" sitting) named this exact write as the
2/2-deterministic trigger.

## Building

This crate is its own Cargo workspace (see the `[workspace]` table in `Cargo.toml`) — it
does not need the rest of the UnaOS repo to build. From this directory:

```sh
rustup toolchain install nightly
rustup component add rust-src --toolchain nightly
rustup target add aarch64-unknown-uefi --toolchain nightly   # usually unnecessary; see below

cargo +nightly build --release --target aarch64-unknown-uefi \
  -Z build-std=core,compiler_builtins,alloc \
  -Z build-std-features=compiler-builtins-mem
```

`aarch64-unknown-uefi` is a Tier-2/3 built-in Rust target (no custom target-json needed);
`-Z build-std` rebuilds `core`/`alloc` for it, which is why a nightly toolchain + the
`rust-src` component are required. This produces
`target/aarch64-unknown-uefi/release/orin-xhci-repro.efi`.

## Running

Copy the `.efi` binary onto a FAT-formatted UEFI boot stick as
`EFI/BOOT/BOOTAA64.EFI` (or launch it from the UEFI Shell / Boot Manager by any other
path) and boot the Orin from it. The app runs entirely over the UEFI text console — no
UnaOS kernel is loaded, and boot services are never exited.

## What to expect

See the accompanying reply draft (`~/.claude/plans/unaos/review/unaos-orin-repro-REPLY.md`
in the UnaOS repo's planning tree, or ask the UnaOS maintainer who is providing this
tool) for the full reliability characterization: the fault is a **per-boot coin flip**, not
a guaranteed-every-time repro. UnaOS's own bench observed the CRCR-write trigger fire
**2/2** boots on the layout it was tested on, and Line-A faults generally at rates from 0/4
to 4/4 depending on kernel/firmware image layout on UnaOS's own builds — budget several
boots on an affected unit, expect the fault within the first few if the unit is affected at
all.

QEMU cannot model this fault (there is no Tegra234 RAS/SNOC/ACI implementation in QEMU's
Tegra support) — this tool has only ever been verified to compile and to read the correct
registers; the actual fault has not been (and cannot be) observed in emulation. The fault
observation is an attended metal run.
