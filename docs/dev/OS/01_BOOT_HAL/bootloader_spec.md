# Bootloader Specification & Handoff

## 1. The UEFI Contract
unaOS requires a UEFI 2.x compliant environment.
* **Entry Point:** The kernel expects to be loaded as an EFI executable (`BOOTX64.EFI`).
* **FrameBuffer:** The bootloader MUST set up the Graphics Output Protocol (GOP) before handing off control. The kernel does not perform mode-setting during early boot (to prevent flickering).

## 2. OpenCore & Mac Hardware Compatibility
To support Apple hardware (specifically MacBookPro10,1 and newer) and patched environments (OpenCore):
* **ACPI Tables:** The kernel will respect ACPI patches applied by OpenCore. We do not re-enumerate the raw hardware; we trust the tables provided by the bootloader.
* **System Integrity:** Secure Boot signatures, if present, are verified against the `unaOS_Root_CA` key.

## 3. Memory Map Handoff
Upon exit of Boot Services:
1.  The bootloader provides a memory map of type `EfiLoaderData`.
2.  The kernel immediately claims all "Conventional Memory" for the `PhysMem` manager.
3.  The kernel engages the "Virtual Memory Air-Gap" (see `docs/02_KERNEL_CORE/memory_model.md`).

## 4. The console the loader logs to, and the 80-column contract
The loader's diagnostics go through the `uefi` crate's logger to ConOut. **The loader never calls
`SetMode`, so the console width is inherited firmware state and must not be assumed.** On the Jetson
Orin it is sometimes 80 columns and sometimes much wider, from the same binary on the same board:
the wrapped and unwrapped captures print the same call sites at the same line numbers.

When the console is 80 columns the firmware's terminal driver **hard-wraps every ConOut write at
column 80 and emits a real CRLF**. No bytes are lost, but the record is split across two physical
lines and the continuation carries no prefix:

```
[ INFO]: crates/bootloader/src/main.rs@743: Kernel ELF: min_vaddr=0x0, max_vaddr
=0x23b968, pages=572
```

Because every reader here is line-oriented (`awk '/pattern/' <log>`), a wrapped record reads as a
truncated one. This is not hypothetical: it cost a full Orin session, during which the image-identity
witness appeared to stop at the word `max_vaddr` while the value was on the wire the whole time.

**Budget.** The logger prefix `[ INFO]: crates/bootloader/src/main.rs@NNN: ` is 44 columns at a
three-digit line number, leaving **36 columns of payload**. Measured on real boots, 14 of the
loader's 15 call sites exceed it; the largest renders at 240.

**Rules.**
* The **image-identity witness must fit in 80 columns** — it is the line that says which image is
  running, and it has to be readable with no tooling in front of it. It is
  `log::info!("KELF min={:#x} max={:#x} pg={}", …)`. Measured on the QEMU wire, not computed:
  **76 columns on aarch64** (`KELF min=0x0 max=0x22f7f8 pg=560`) and **78 on x86_64**
  (`KELF min=0x0 max=0x127902f pg=4730`, whose larger kernel already spends a seventh hex digit and
  a fourth page digit). x86_64 is the binding case and it has **2 columns of headroom**. Do not
  lengthen this line, and do not add a field to it.
* Other call sites may exceed the budget. Read those captures through
  `~/unaos-bench/tools/unwrap80.sh <log>`, which rejoins the wraps (and is a no-op on a capture
  taken with a wide console).

## 5. Build gating
`crates/bootloader` is type-checked by `./arroyo check` under **GATE-BOOTLOADER**: the
`aarch64-unknown-uefi` and `x86_64-unknown-uefi` default legs, plus a `bootdiag,jb8lever` feature
cross that no default leg on either arch compiles. Before that gate existed, `check` ran `cargo
check` inside `crates/kernel` and nowhere else, and every loader edit went into a commit whose DONE
gate could not see it.
