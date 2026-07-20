# Cleanroom Specification: NVIDIA Kepler (GK107) PGRAPH Microcode

> [!WARNING]
> **CLEANROOM POLICY NOTICE**
> NO proprietary firmware blobs may enter the UnaOS source tree. Extracting and distributing the NVIDIA microcode from macOS binaries is strictly prohibited. This document specifies only the neutral hardware interfaces and behavioral requirements of the Falcon microcontroller. The target is a 100% from-scratch, open-source reimplementation of the initialization firmware.

## 1. The Falcon Microcontroller

NVIDIA GPUs from the Fermi/Kepler era utilize custom 32-bit microcontrollers called **Falcon** (FAst Logic CONtroller) to manage complex engines. The `PGRAPH` block (which handles 2D, 3D, and Compute) is managed by one such Falcon processor.

When the GPU boots, `PGRAPH` is in a reset state. It refuses to process any commands from the PFIFO pushbuffer until its embedded Falcon microcontroller boots, runs an initialization routine to set up the internal pipeline state, and signals that it is ready.

## 2. Firmware Upload Interface (Host OS)

To load firmware into the Falcon, the host OS must upload the payload into the Falcon's Instruction Memory (IMEM) and Data Memory (DMEM) using MMIO registers mapped in BAR0.

### PGRAPH Falcon Registers (Base `0x400000`)
- **IMEM Upload**:
  - `NV_PGRAPH_FALCON_IMEMC` (`0x400180`): Instruction Memory Control (set upload offset here, e.g., `(offset >> 8) | 1 << 24` for auto-increment).
  - `NV_PGRAPH_FALCON_IMEMD` (`0x400184`): Instruction Memory Data (write 32-bit firmware instructions here).
- **DMEM Upload**:
  - `NV_PGRAPH_FALCON_DMEMC` (`0x4001C0`): Data Memory Control.
  - `NV_PGRAPH_FALCON_DMEMD` (`0x4001C4`): Data Memory Data.
- **Execution Control**:
  - `NV_PGRAPH_FALCON_BOOTVEC` (`0x400104`): Boot vector (starting instruction offset).
  - `NV_PGRAPH_FALCON_CPUCTL` (`0x400100`): CPU Control (Write `2` to start execution).

## 3. What the Microcode Must Do

To successfully wake up `PGRAPH`, the open-source firmware must implement the following behaviors:

1. **Pipeline Initialization**: The microcode must write specific values to internal, undocumented `PGRAPH` state registers to clear invalid states left by a hardware reset. 
2. **Context Switching (CTXPROG)**: When the host OS switches between different GPU channels (e.g., two different applications drawing at once), the Falcon microcode must save the 3D pipeline state of the old channel to VRAM and restore the state of the new channel.
3. **Interrupt Handling**: The Falcon must handle trapping illegal 3D commands, page faults (if using virtual memory), and synchronization fences, reporting them back to the host OS via the `NV_PMC_INTR` master interrupt tree.

## 4. Development Toolchain

An open-source toolchain exists for the Falcon ISA, created by the Nouveau project:

- **Envytools (`envyas`)**: An assembler that can compile Falcon assembly (`.fuc` files) into the binary format required by IMEM.
- **Instruction Set**: The Falcon is a 32-bit architecture with 16 general-purpose registers (`$r0` - `$r15`), a stack pointer (`$sp`), and a program counter (`$pc`). It supports standard ALU operations, branching, and a specific `iowr` / `iord` instruction pair for accessing the GPU's internal registers.


